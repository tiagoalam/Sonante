//! Windows-only read-only catalog. No MPD, audio or tag writes during scanning.
use super::protocol::Result;
use base64::Engine;
use lofty::{
    config::ParseOptions,
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

const MAX_TRACKS: usize = 100_000;
#[path="library_scan_io.rs"]
mod scan_io;
#[path="library_snapshot.rs"]
mod snapshot;
#[path="library_import.rs"]
pub(crate) mod imports;
#[path="library_ab.rs"]
mod album_ab;
#[path="library_missing.rs"]
pub(crate) mod missing;
fn id(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn audio(path: &Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        [
            "flac", "wav", "mp3", "ogg", "opus", "m4a", "aac", "aif", "aiff", "wv",
        ]
        .contains(&v.to_ascii_lowercase().as_str())
    })
}
fn linked(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
fn root_label(roots: &[PathBuf]) -> Option<String> {
    roots.first().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Music".into()))
}
fn root_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.to_str().is_none_or(|s| s.len() > 32768 || s.contains(['\0','\r','\n'])) || path.components().any(|p| matches!(p, std::path::Component::ParentDir)) { return Err("unsupported_path"); }
    crate::windows_file_info::admit_local_root(path).map_err(|_| "unsupported_path")
}
fn canonical_root(path: &Path) -> Result<PathBuf> {
    let path=path.to_path_buf();
    crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
        if !job.current(){return Err("index_timeout");}
        let result=canonical_root_read(&path,||job.current());
        if !job.current(){return Err("index_timeout");}
        result
    }).map_err(io_wait_error)?
}
fn io_wait_error(error:crate::windows_io::Error)->&'static str {match error{
    crate::windows_io::Error::Busy=>"engine_busy",
    crate::windows_io::Error::Timeout=>"index_timeout",
    crate::windows_io::Error::WorkerFailed=>"worker_failed",
}}
fn canonical_root_read(path: &Path,current:impl Fn()->bool) -> Result<PathBuf> {
    if !current(){return Err("index_timeout");}
    root_path(path)?;
    if !current(){return Err("index_timeout");}
    let normalized=crate::windows_file_info::normalize_root(path).map_err(|_|{crate::diagnostics::network_failure(crate::diagnostics::NetworkStep::Normalize,None);"windows_network_unavailable"})?;
    if !current(){return Err("index_timeout");}
    root_path(&normalized)?;
    let path = normalized.canonicalize().map_err(|error| {crate::diagnostics::network_failure(crate::diagnostics::NetworkStep::Canonicalize,error.raw_os_error().and_then(|n|u32::try_from(n).ok()));"library_unavailable"})?;
    if !current(){return Err("index_timeout");}
    root_path(&path)?;
    crate::windows_file_info::verify_network_root(&path).map_err(|_|{crate::diagnostics::network_failure(crate::diagnostics::NetworkStep::Root,None);"windows_network_unavailable"})?;
    if !current(){return Err("index_timeout");}
    if !path.is_dir() {
        return Err("library_unavailable");
    }
    if !current(){return Err("index_timeout");}
    Ok(path)
}
fn disc_folder(path: &Path) -> Option<u32> {
    let name = path
        .file_name()?
        .to_str()?
        .to_lowercase()
        .replace([' ', '-', '_'], "");
    ["disc", "disk", "disco", "cd"]
        .into_iter()
        .find_map(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|v| *v > 0 && *v <= 999)
        })
}
struct Budget<R> {
    file: R,
    remaining: usize,
}
impl<R: Read> Read for Budget<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("metadata_budget"));
        }
        let limit = buf.len().min(self.remaining);
        let n = self.file.read(&mut buf[..limit])?;
        self.remaining -= n;
        Ok(n)
    }
}
impl<R: Seek> Seek for Budget<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

// One live transport per queue occurrence. It is never deserialized or stored.
pub(crate) trait RemoteLease: Send + Sync {
    fn uri(&self) -> &str;
    fn source(&self) -> &super::queue_v3::Source;
    fn connection_id(&self) -> &str;
    fn healthy(&self) -> bool;
    // Already-received metadata only; never a playback clock or extra I/O.
    fn presentation_duration(&self) -> Option<f64> { None }
    fn health_signal(&self)->Option<Arc<AtomicBool>>{None}
    fn command(&self, work: &mut dyn FnMut()->super::protocol::Result<Vec<String>>) -> super::protocol::Result<Vec<String>>;
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub disc: u32,
    pub number: u32,
    pub file_name: String,
    #[serde(default)]
    pub catalog_album_id: Option<String>,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub media_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio: Option<crate::windows_radio::Playing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upnp: Option<crate::windows_upnp::media::Playing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_source: Option<super::queue_v3::Source>,
    // Ephemeral navigation context; the v3 disk codec never serializes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plex_image:Option<crate::plex::PlexImageRef>,
    #[serde(skip)]
    pub(crate) remote_lease: Option<Arc<dyn RemoteLease>>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub track_count: usize,
    pub has_cover: bool,
    pub added_at: Option<u64>,
}
#[derive(Clone)]
struct Record {
    root: PathBuf,
    album: Album,
    tracks: Vec<Track>,
    folder: PathBuf,
    cover: Option<PathBuf>,
    search: String,
    // Presentation only; not part of Track, the playback queue or its cache.
    durations: HashMap<String, f64>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub epoch: u64,
    pub revision: u64,
    pub phase: String,
    pub root_name: Option<String>,
    pub albums: usize,
    pub tracks: usize,
    pub warnings: usize,
    pub metadata_warnings: usize,
    pub error: Option<&'static str>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Albums {
    pub epoch: u64,
    pub revision: u64,
    pub total: usize,
    pub items: Vec<Album>,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Sources { pub local:bool, pub navidrome:bool, #[serde(default)] pub plex:bool }
impl Default for Sources {fn default()->Self{Self{local:true,navidrome:false,plex:false}}}
impl Sources {pub fn validate(self)->Result<()> {if !self.local&&!self.navidrome&&!self.plex{Err("windows_source_required")}else{Ok(())}}}
// Mirrors the existing Linux0.6.2 preference; no platform-specific mode.
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageMode { #[default] Full, Simple }
fn is_full_mode(value:&UsageMode)->bool{*value==UsageMode::Full}
#[derive(Clone)]
struct State {
    usage_mode: UsageMode,
    online_artwork_enabled: bool,
    experimental_features: bool,
    editions_revision: u64,
    first_run: bool,
    catalog_rebuild_attempted: bool,
    catalog_generation: Option<u64>,
    sources: Sources,
    signature: Option<[u8; 32]>,
    // A snapshot digest is only a baseline for an explicit future check.
    // It is deliberately distinct from the signature of a currently scanned catalog.
    cached_signature: Option<[u8; 32]>,
    root_summaries: BTreeMap<PathBuf, source_scope::Summary>,
    added_error: bool,
    status: Status,
    root: Option<PathBuf>,
    roots: Vec<PathBuf>,
    albums: HashMap<String, Record>,
}
#[derive(Clone)]
pub struct Library {
    data: Arc<Mutex<State>>,
    pub(crate) imports: Arc<imports::Coordinator>,
    epoch: Arc<AtomicU64>,
    invalid_before: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    config: PathBuf,
    change: Arc<Mutex<()>>,
    checks: Arc<check::Signals>,
    legacy_added_roots: Arc<Mutex<std::collections::BTreeSet<String>>>,
    setup_initialized: Arc<AtomicBool>,
    settings_current: Arc<AtomicBool>,
    settings_fingerprint: Arc<Mutex<Option<[u8;32]>>>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    version: u8,
    #[serde(default,skip_serializing_if="is_full_mode")]
    usage_mode: UsageMode,
    #[serde(default)]
    online_artwork_enabled: bool,
    #[serde(default)]
    experimental_features: bool,
    #[serde(default, skip_serializing_if="Option::is_none")]
    first_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
    #[serde(default)]
    roots: Vec<PathBuf>,
    #[serde(default)]
    sources: Option<Sources>,
    #[serde(default)]
    catalog_rebuild_attempted: bool,
    #[serde(default)]
    catalog_generation: Option<u64>,
}
impl Saved {
    fn folders(self) -> Result<Vec<PathBuf>> {
        match self.version {
            1 if self.roots.is_empty() => Ok(vec![self.root.ok_or("library_settings")?]),
            2 | 3 if self.root.is_none() && self.roots.len() <= 32 => Ok(self.roots),
            _ => Err("library_settings"),
        }
    }
}
pub struct Selection {
    pub root: PathBuf,
    pub tracks: Vec<Track>,
    pub start: usize,
    pub album_id: Option<String>,
}

impl Library {
    pub fn new(config: PathBuf) -> Self { Self::open(config, Vec::new()) }
    pub fn with_legacy(config: PathBuf, legacy: PathBuf) -> Self { Self::open(config, vec![legacy]) }
    pub fn with_legacy_chain(config:PathBuf,legacy:Vec<PathBuf>)->Self{Self::open(config,legacy)}
    fn open(config: PathBuf, legacy: Vec<PathBuf>) -> Self {
        let config_clock=std::time::Instant::now();
        let absent = |path: &Path| fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        let restore = if !absent(&config) { Some(config.clone()) } else { legacy.into_iter().find(|path|!absent(path)) };
        let settings_current=!absent(&config);
        let this = Self {
            imports: Arc::new(imports::Coordinator::default()),
            data: Arc::new(Mutex::new(State {
                usage_mode:UsageMode::Full, online_artwork_enabled:false, experimental_features: false, editions_revision: 0,
                first_run: restore.is_none(),
                catalog_rebuild_attempted: false,
                catalog_generation: None,
                sources: Sources::default(),
                signature: None,
                cached_signature: None,
                root_summaries: BTreeMap::new(),
                added_error: false,
                status: Status {
                    epoch: 0,
                    revision: 0,
                    phase: "idle".into(),
                    root_name: None,
                    albums: 0,
                    tracks: 0,
                    warnings: 0,
                    metadata_warnings: 0,
                    error: None,
                },
                root: None,
                roots: Vec::new(),
                albums: HashMap::new(),
            })),
            epoch: Arc::new(AtomicU64::new(0)),
            invalid_before: Arc::new(AtomicU64::new(0)),
            stopping: Arc::new(AtomicBool::new(false)),
            config,
            change: Arc::new(Mutex::new(())),
            checks: Arc::new(check::Signals::default()),
            legacy_added_roots: Arc::new(Mutex::new(Default::default())),
            setup_initialized: Arc::new(AtomicBool::new(restore.is_some())),
            settings_current: Arc::new(AtomicBool::new(settings_current)),
            settings_fingerprint: Arc::new(Mutex::new(None)),
        };
        if let Some(restore) = restore {
            let restored = (|| -> Result<(Vec<PathBuf>,Sources,bool,bool,bool,UsageMode,bool,Option<u64>)> {
                let metadata=fs::symlink_metadata(&restore).map_err(|_|"library_settings")?;
                if !metadata.is_file()||linked(&metadata){return Err("library_settings");}
                if metadata.len()>1024*1024+16384{return Err("library_settings");}
                let mut bytes=Vec::new();File::open(&restore).map_err(|_|"library_settings")?.take(1024*1024+16385).read_to_end(&mut bytes).map_err(|_|"library_settings")?;
                if bytes.len()>1024*1024+16384{return Err("library_settings");}
                let saved:Saved=serde_json::from_slice(&bytes).map_err(|_|"library_settings")?;
                let sources=saved.sources.unwrap_or_default();sources.validate()?;
                let usage_mode=saved.usage_mode;let first_run=saved.first_run.unwrap_or(false);let experimental=saved.experimental_features;let online=saved.online_artwork_enabled;let attempted=saved.catalog_rebuild_attempted;let generation=saved.catalog_generation;let roots=saved.folders()?;
                if restore==this.config{*this.settings_fingerprint.lock().map_err(|_|"library_state")?=Some(Sha256::digest(&bytes).into());}
                Ok((roots,sources,first_run,experimental,online,usage_mode,attempted,generation))
            })();
            let restored = restored.and_then(|(roots,sources,first_run,experimental,online,usage_mode,catalog_rebuild_attempted,catalog_generation)| {
                *this.legacy_added_roots.lock().map_err(|_| "library_state")? = roots.iter().map(|r| id(&r.to_string_lossy())).collect();
                {
                    let mut s = this.data.lock().map_err(|_| "library_state")?;
                    s.root = roots.first().cloned();
                    s.roots = roots.clone();
                    s.sources=sources;
                    s.first_run=first_run;s.catalog_rebuild_attempted=catalog_rebuild_attempted;s.catalog_generation=catalog_generation;s.experimental_features=experimental;s.online_artwork_enabled=online;s.usage_mode=usage_mode;
                    s.status.root_name = root_label(&roots);
                }
                if !sources.local {
                    this.data.lock().map_err(|_|"library_state")?.status.phase="ready".into();Ok(this.status()?)
                }else{
                        match snapshot::load_for_generation(&this.config, &roots, catalog_generation) {
                        Ok(snapshot::LoadResult::Restored(cached)) => {
                            let mut state = this.data.lock().map_err(|_| "library_state")?;
                            state.albums = cached.records;
                            state.cached_signature = Some(cached.signature);
                            state.catalog_generation = Some(cached.generation);
                            state.status.phase = "cached".into();
                            state.status.revision = 1;
                            state.status.albums = state.albums.len();
                            state.status.tracks = state.albums.values().map(|record| record.tracks.len()).sum();
                            state.status.warnings = cached.warning_counts.total();
                            state.status.metadata_warnings = cached.warning_counts.metadata_decode;
                            state.status.error = cached.provisional.then_some("windows_library_check_unconfirmed");
                            Ok(state.status.clone())
                        }
                        Ok(snapshot::LoadResult::Missing | snapshot::LoadResult::LegacyV1) if !catalog_rebuild_attempted && !roots.is_empty() => {
                            if this.set_catalog_rebuild_attempted(true).is_ok() { this.startup_scan(roots) }
                            else { Err("windows_catalog_rebuild_marker_failed") }
                        }
                        Ok(snapshot::LoadResult::Missing | snapshot::LoadResult::LegacyV1) if catalog_rebuild_attempted => {
                            let mut state = this.data.lock().map_err(|_| "library_state")?;
                            state.status.phase = "failed".into();
                            state.status.error = Some("windows_catalog_rebuild_requires_manual_retry");
                            Ok(state.status.clone())
                        }
                        Ok(snapshot::LoadResult::Missing) => this.startup_scan(roots),
                        Ok(snapshot::LoadResult::LegacyV1) => Err("library_empty"),
                        Ok(snapshot::LoadResult::Incompatible) | Err(_) => {
                            eprintln!("Windows library: catalog_snapshot_load_refused");
                            let mut state = this.data.lock().map_err(|_| "library_state")?;
                            state.status.phase = "failed".into();
                            state.status.error = Some("windows_catalog_recovery_required");
                            Ok(state.status.clone())
                        }
                    }
                }
            });
            match restored {
                Ok(_) => {}
                Err(code) => {
                    if let Ok(mut s) = this.data.lock() {
                        s.status.phase = "failed".into();
                        s.status.error = Some(code)
                    }
                }
            }
        }
        // Passive restore observation only; failed settings are preserved.
        let restore_error=this.data.lock().ok().and_then(|s|s.status.error);
        crate::local_diagnostics::record(crate::local_diagnostics::Stage::StartupConfig,config_clock.elapsed(),restore_error);
        this
    }
    fn startup_scan(&self, roots: Vec<PathBuf>) -> Result<Status> {
        // Own the initial catalog job before its worker starts. A valid snapshot
        // bypasses this path entirely and remains provisional until checked.
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::ScanStarted {
                origin: crate::local_diagnostics::LibraryScanOrigin::StartupSnapshotUnavailable,
                root_count: roots.len(),
            }, None);
        let owner = self.imports.admit(true, true)?;
        let result = {
            let _change = self.change.lock().map_err(|_| "library_state")?;
            self.begin_locked_import_preferences(roots, false, None, None, None, None, Some(&owner))
        };
        match result {
            Ok(status) => { owner.commit()?; Ok(status) }
            Err(code) => { owner.fail(code); Err(code) }
        }
    }
    pub fn stop(&self) {
        self.imports.shutdown();
        self.stopping.store(true, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        // Confirmation uses its own short signal gate, never the catalog lock.
        self.checks.notify();
    }
    pub fn status(&self) -> Result<Status> {
        Ok(self
            .data
            .lock()
            .map_err(|_| "library_state")?
            .status
            .clone())
    }
    pub fn select(&self, root: PathBuf) -> Result<Status> { self.select_roots(vec![root]) }
    pub fn select_roots(&self, roots: Vec<PathBuf>) -> Result<Status> {
        self.imports.require_idle()?;
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::ScanStarted {
                origin: crate::local_diagnostics::LibraryScanOrigin::RootConfiguration,
                root_count: roots.len(),
            }, None);
        self.begin_many(roots, true)
    }
    pub fn refresh(&self) -> Result<Status> {
        self.imports.require_idle()?;
        let _change = self.change.lock().map_err(|_| "library_state")?;
        self.imports.require_idle()?;
        let (roots, cached) = {
            let state = self.data.lock().map_err(|_| "library_state")?;
            (state.roots.clone(), state.status.phase == "cached")
        };
        if roots.is_empty() { return Err("library_empty"); }
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::ScanStarted {
                origin: crate::local_diagnostics::LibraryScanOrigin::ExplicitRefresh,
                root_count: roots.len(),
            }, None);
        if cached { return self.begin_cached_scan_locked(); }
        self.begin_locked(roots, false)
    }
    fn begin_many(&self, roots: Vec<PathBuf>, persist: bool) -> Result<Status> {
        let _change = self.change.lock().map_err(|_| "library_state")?;
        self.imports.require_idle()?;
        self.begin_locked(roots, persist)
    }
    // Folder choice and refresh resolve their root under the same change lock.
    fn begin_locked(&self, roots: Vec<PathBuf>, persist: bool) -> Result<Status> {self.begin_locked_sources(roots,persist,None)}
    fn begin_locked_sources(&self, roots: Vec<PathBuf>, persist: bool, sources:Option<Sources>) -> Result<Status> {self.begin_locked_options(roots,persist,sources,None)}
    fn begin_locked_options(&self, roots:Vec<PathBuf>,persist:bool,sources:Option<Sources>,experimental:Option<bool>)->Result<Status>{self.begin_locked_features(roots,persist,sources,experimental,None)}
    fn begin_locked_features(&self,roots:Vec<PathBuf>,persist:bool,sources:Option<Sources>,experimental:Option<bool>,online:Option<bool>)->Result<Status>{self.begin_locked_preferences(roots,persist,sources,experimental,online,None)}
    fn begin_locked_preferences(&self,roots:Vec<PathBuf>,persist:bool,sources:Option<Sources>,experimental:Option<bool>,online:Option<bool>,mode:Option<UsageMode>)->Result<Status>{
        self.begin_locked_import_preferences(roots,persist,sources,experimental,online,mode,None)
    }
    fn begin_locked_import_preferences(&self,roots:Vec<PathBuf>,persist:bool,sources:Option<Sources>,experimental:Option<bool>,online:Option<bool>,mode:Option<UsageMode>,owner:Option<&imports::Guard>)->Result<Status>{
        let usage_mode=mode.unwrap_or(self.usage_mode()?);
        let online=online.unwrap_or(self.online_artwork_enabled()?);
        let experimental=experimental.unwrap_or(self.experimental_features()?);
        let sources=sources.unwrap_or(self.data.lock().map_err(|_|"library_state")?.sources);sources.validate()?;
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        if roots.len() > 32 { return Err("windows_folder_limit"); }
        // Read-only admission owns only copied paths, never settings/catalog
        // locks or publication authority. A late OS result cannot save roots.
        let roots=crate::windows_io::read(std::time::Duration::from_secs(6),move|job|{
            let mut admitted=Vec::new();
            for root in roots {
                if !job.current(){return Err("index_timeout");}
                let root=if persist{canonical_root_read(&root,||job.current())?}else{root};
                root_path(&root)?;
                if admitted.iter().any(|other:&PathBuf|root.starts_with(other)||other.starts_with(&root)){return Err("windows_folders_overlap");}
                admitted.push(root);
            }
            if admitted.len()==1&&!persist{canonical_root_read(&admitted[0],||job.current())?;}
            if !job.current(){return Err("index_timeout");}
            Ok(admitted)
        }).map_err(io_wait_error)??;
        if self.stopping.load(Ordering::Acquire){return Err("shutting_down");}
        // Reserve actual scan capacity BEFORE changing persisted/visible roots.
        // An unreturned OS scan retains its slot even after logical timeout.
        let scan_permit = if sources.local { Some(scan_io::Admission::acquire()?) } else { None };
        // Persist the admitted folder set before replacing the visible catalog.
        if persist {
            self.store_settings(&Saved {
                    version: 3, usage_mode, experimental_features:experimental,online_artwork_enabled:online,
                    first_run:Some(self.first_run()?),
                    root: None,
                    roots: roots.clone(),
                    sources:Some(sources),
                    catalog_rebuild_attempted: self.data.lock().map_err(|_| "library_state")?.catalog_rebuild_attempted,
                    catalog_generation: self.data.lock().map_err(|_| "library_state")?.catalog_generation,
                })?;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(owner)=owner { owner.bind(epoch,&roots)?; } else { self.imports.supersede(epoch); }
        {
            let mut s = self.data.lock().map_err(|_| "library_state")?;
            s.root = roots.first().cloned();
            s.roots = roots.clone();
            s.sources=sources;s.experimental_features=experimental;s.online_artwork_enabled=online;s.usage_mode=usage_mode;s.editions_revision=s.editions_revision.wrapping_add(1);
            s.albums.clear();
            s.signature = None;
            s.cached_signature = None;
            s.root_summaries.clear();
            s.added_error = false;
            s.status = Status {
                epoch,
                revision: 0,
                phase: "scanning".into(),
                root_name: root_label(&roots),
                albums: 0,
                tracks: 0,
                warnings: 0,
                metadata_warnings: 0,
                error: None,
            };
            self.checks.notify();
        }
        if !sources.local {
            self.data.lock().map_err(|_|"library_state")?.status.phase="ready".into();
            return self.status();
        }
        let worker = self.clone();
        if std::thread::Builder::new()
            .name("sonante-windows-library".into())
            .spawn(move || {
                let _permit=scan_permit;
                let _completion=worker.imports.worker(epoch);
                let failed=worker.clone();
                let scope=match scan_io::Scope::start(move||failed.scan_failed(epoch,"index_timeout")){
                    Ok(scope)=>scope,Err(_)=>{worker.scan_failed(epoch,"worker_failed");return;}
                };
                worker.scan(roots, epoch, scope, None);
            })
            .is_err()
        {
            self.imports.finish(epoch,Some("worker_failed"));
            let mut s = self.data.lock().map_err(|_| "library_state")?;
            s.status.phase = "failed".into();
            s.status.error = Some("worker_failed");
            self.checks.notify();
            return Err("worker_failed");
        }
        self.status()
    }
    pub(crate) fn current(&self, epoch: u64) -> bool {
        !self.stopping.load(Ordering::Acquire) && self.epoch.load(Ordering::Acquire) == epoch
            && epoch >= self.invalid_before.load(Ordering::Acquire)
    }
    fn scan_failed(&self, epoch:u64, code:&'static str) {
        let diagnostic_code = match code {
            "windows_network_unavailable" | "library_unavailable" => crate::local_diagnostics::LibraryScanFailure::NetworkUnavailable,
            source_scope::OTHER_UNAVAILABLE => crate::local_diagnostics::LibraryScanFailure::OtherSourceUnavailable,
            "index_timeout" => crate::local_diagnostics::LibraryScanFailure::Timeout,
            "worker_failed" => crate::local_diagnostics::LibraryScanFailure::WorkerFailed,
            "scope_changed" | "library_changed" => crate::local_diagnostics::LibraryScanFailure::ScopeChanged,
            _ => crate::local_diagnostics::LibraryScanFailure::Other,
        };
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::ScanFailed { code: diagnostic_code }, Some(code));
        self.imports.finish(epoch,Some(code));
        // Invalidate publication before waiting for a catalog lock. A stalled
        // OS worker can return later, but can never revive this generation.
        self.invalid_before.fetch_max(epoch.saturating_add(1),Ordering::AcqRel);
        if let Ok(mut state)=self.data.lock(){
            if state.status.epoch==epoch && state.status.phase=="scanning" {
                state.status.phase=if state.cached_signature.is_some(){"cached"}else{"failed"}.into();state.status.error=Some(code);
                state.signature=None;
            }
        }
        self.checks.notify();
    }
    fn scan(&self, roots: Vec<PathBuf>, epoch: u64, scope:scan_io::Scope, destination: Option<&Library>) {
        let scanned_roots = roots.clone();
        let mut summaries = match self.data.lock() { Ok(s) => s.root_summaries.clone(), Err(_) => return };
        let retained_tracks: usize = match self.data.lock() { Ok(s) => s.albums.values().map(|a| a.tracks.len()).sum(), Err(_) => return };
        let mut signatures: BTreeMap<_, _> = roots.iter().cloned().map(|r| (r, (check::Signature::new(), 0usize))).collect();
        let mut stack: Vec<_> = roots.into_iter().rev().map(|root| (root.clone(), root, 0usize)).collect();
        let mut batch = Vec::new();
        let mut warnings = crate::local_diagnostics::LibraryWarningCounts::default();
        let mut warnings_by_root: BTreeMap<PathBuf, crate::local_diagnostics::LibraryWarningCounts> =
            scanned_roots.iter().cloned().map(|root| (root, Default::default())).collect();
        let mut tracks = retained_tracks;
        let mut entries_seen = summaries.values().map(|s| s.entries).sum::<usize>();
        let mut limit = false;
        let mut covers: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
        while let Some((dir, root, depth)) = stack.pop() {
            if depth==0 && crate::windows_file_info::network_root(&root){
                if !self.current(epoch)||!scope.checkpoint(){return;}
                if crate::windows_file_info::verify_network_root(&root).is_err(){self.scan_failed(epoch,"windows_network_unavailable");return;}
            }
            if !self.current(epoch) || !scope.checkpoint() {
                return;
            }
            if depth > 64 {
                warnings.depth_limit += 1;
                warnings_by_root.entry(root.clone()).or_default().depth_limit += 1;
                continue;
            }
            let canonical = match dir.canonicalize() {
                Ok(p) if p.starts_with(&root) => p,
                _ => {
                    warnings.directory_path += 1;
                    warnings_by_root.entry(root.clone()).or_default().directory_path += 1;
                    continue;
                }
            };
            if !self.current(epoch)||!scope.checkpoint(){return;}
            let directory_guard=if crate::windows_file_info::network_root(&root){
                match crate::windows_file_info::ContentDirectory::capture(&root,&canonical,||self.current(epoch)&&scope.current()){
                    Ok(guard)=>Some(guard),Err(_)=>{self.scan_failed(epoch,"windows_network_unavailable");return;}
                }
            }else{None};
            if !self.current(epoch)||!scope.checkpoint(){return;}
            let entries = match fs::read_dir(&canonical) {
                Ok(v) => v,
                Err(_) => {
                    warnings.directory_read += 1;
                    warnings_by_root.entry(root.clone()).or_default().directory_read += 1;
                    continue;
                }
            };
            let mut entries=entries;
            loop {
                if !self.current(epoch)||!scope.checkpoint(){return;}
                let entry=entries.next();
                if !self.current(epoch)||!scope.checkpoint(){return;}
                let Some(entry)=entry else{break;};
                entries_seen += 1;
                let Some((signature, entry_count)) = signatures.get_mut(&root) else { self.scan_failed(epoch,"worker_failed"); return; };
                *entry_count += 1;
                if entries_seen > 500_000 {
                    limit = true;
                    break;
                }
                if !self.current(epoch) || !scope.checkpoint() {
                    return;
                }
                let entry = match entry {
                    Ok(v) => v,
                    Err(_) => {
                        warnings.entry_read += 1;
                        warnings_by_root.entry(root.clone()).or_default().entry_read += 1;
                        continue;
                    }
                };
                let path = entry.path();
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(v) => v,
                    Err(_) => {
                        warnings.metadata_read += 1;
                        warnings_by_root.entry(root.clone()).or_default().metadata_read += 1;
                        continue;
                    }
                };
                if !self.current(epoch)||!scope.checkpoint(){return;}
                let signature_slot = signature.observe(&path, &metadata);
                if linked(&metadata) {
                    continue;
                }
                if metadata.is_dir() {
                    stack.push((path, root.clone(), depth + 1));
                    continue;
                }
                if !metadata.is_file() || !audio(&path) {
                    continue;
                }
                let path = match path.canonicalize() {
                    Ok(v) if v.starts_with(&root) => v,
                    _ => {
                        warnings.file_containment += 1;
                        warnings_by_root.entry(root.clone()).or_default().file_containment += 1;
                        continue;
                    }
                };
                if tracks >= MAX_TRACKS {
                    limit = true;
                    break;
                }
                if !self.current(epoch)||!scope.checkpoint(){return;}
                let active=||self.current(epoch)&&scope.current();
                let (mut record, warn) = metadata_record(&path, &root, &active);
                if !active()||!scope.checkpoint(){return;}
                record.cover = covers
                    .entry(record.folder.clone())
                    .or_insert_with(|| find_cover(&record.folder, &active))
                    .clone();
                if !active()||!scope.checkpoint(){return;}
                record.album.has_cover = record.cover.is_some();
                signature.record(signature_slot, &record);
                let metadata_warning = usize::from(warn);
                warnings.metadata_decode += metadata_warning;
                warnings_by_root.entry(root.clone()).or_default().metadata_decode += metadata_warning;
                batch.push(record);
                tracks += 1;
                if !scope.current(){return;}
                if batch.len() >= 32 && !self.publish(epoch, &mut batch, warnings.total(), warnings.metadata_decode, false, limit, None) {
                    return;
                }
            }
            // ReadDir's OS close belongs to the same physical scan slot.
            if !scope.checkpoint(){return;}
            drop(entries);
            if !self.current(epoch)||!scope.checkpoint(){return;}
            drop(directory_guard);
            if !self.current(epoch)||!scope.checkpoint(){return;}
            if limit { break; }
        }
        // All share handles/enumerators have physically returned before the
        // local admission journal is allowed to publish a completed scan.
        if !self.current(epoch)||!scope.checkpoint(){return;}
        // A scoped check retains other roots from the qualified catalog, but
        // validates their published targets without discovering or parsing them.
        if self.validate_retained_sources(&scanned_roots, epoch, &scope).is_err() {
            self.scan_failed(epoch, source_scope::OTHER_UNAVAILABLE); return;
        }
        if !self.current(epoch)||!scope.finish(){return;}
        drop(scope);
        let warning_count = warnings.total();
        let mut structurally_complete = !limit && warnings.structural_total() == 0;
        for (root, (signature, entries)) in signatures {
            if let Some(hash) = signature.finish() { summaries.insert(root, source_scope::Summary { hash, entries }); }
            else { structurally_complete = false; }
        }
        let scan_signature = if structurally_complete {
            match self.data.lock() { Ok(s) => source_scope::signature(&s.roots, &summaries), Err(_) => return }
        } else { None };
        structurally_complete &= scan_signature.is_some();
        // A metadata-only warning permits a snapshot baseline, but never a
        // live confirmation signature or a successful library check.
        let signature = scan_signature.filter(|_| warning_count == 0);
        // Keep progressive publication; only final admission metadata waits for I/O.
        if !self.publish(epoch, &mut batch, warning_count, warnings.metadata_decode, false, limit, None) { return; }
        self.imports.validating(epoch);
        let Ok(_change) = self.change.lock() else { return; };
        if !self.current(epoch) { return; }
        let (roots, rows) = {
            let Ok(s) = self.data.lock() else { return; };
            (scanned_roots.clone(), s.albums.values().filter(|a| scanned_roots.contains(&a.root)).flat_map(|a| a.tracks.iter().map(|t| (t.id.clone(), id(&a.root.to_string_lossy())))).collect::<Vec<_>>())
        };
        let dates = (|| {
            let legacy = self.legacy_added_roots.lock().map_err(|_| "library_state")?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "library_added_unavailable")?.as_secs();
            added::observe(&self.config, &legacy, &roots, &rows, signature.is_some(), now, || self.current(epoch))
        })();
        if !self.current(epoch) { return; }
        let added_ok=dates.is_ok();
        {
            let Ok(mut s) = self.data.lock() else { return; };
            s.root_summaries = if signature.is_some() { summaries } else { BTreeMap::new() };
            s.added_error = dates.is_err();
            if let Ok(dates) = dates {
                for a in s.albums.values_mut() {
                    a.album.added_at = a.tracks.iter().filter_map(|t| dates.get(&t.id).copied().flatten()).min();
                }
            }
        }
        let mut published = if destination.is_some() {
            self.publish(
                epoch,
                &mut batch,
                warning_count,
                warnings.metadata_decode,
                true,
                limit,
                signature,
            )
        } else {
            self.publish(
                epoch,
                &mut batch,
                warning_count,
                warnings.metadata_decode,
                false,
                limit,
                None,
            )
        };
        if published && destination.is_none() {
            // Do not expose `ready` before the final snapshot write attempt.
            // Shutdown immediately after observing `ready` must not retire
            // this complete scan before its provisional/clean snapshot lands.
            self.persist_snapshot_after_scan(
                scan_signature,
                warnings,
                &warnings_by_root,
                structurally_complete,
                added_ok,
            );
            published = self.finish_scan_status(
                epoch,
                warning_count,
                warnings.metadata_decode,
                limit,
                signature,
            );
        }
        if published {
            let counts = self.data.lock().ok().map(|state| (state.status.albums, state.status.tracks));
            if let Some((album_count, track_count)) = counts {
                crate::local_diagnostics::record_library_event(
                    crate::local_diagnostics::LibraryEvent::ScanFinished {
                        album_count, track_count, limited: limit, warnings,
                    }, if signature.is_some() && !limit && warning_count == 0 && added_ok { None } else { Some("windows_library_check_unconfirmed") });
            }
            self.imports.finish(epoch,if signature.is_some()&&!limit&&warning_count==0&&added_ok {None}else{Some("windows_library_check_unconfirmed")});
        }
        // The same change lock covers Added qualification and the live scoped
        // publication. Folder/mode transactions cannot slip between them.
        if let Some(live) = destination {
            // Commit the verified snapshot before waking callers that observe
            // the live catalog as ready. Otherwise a restart can race this
            // write and restore the previous snapshot generation.
            self.persist_snapshot_if_confirmed();
            live.finish_source_scan_locked(self, epoch);
        }
    }
    fn finish_scan_status(
        &self,
        epoch: u64,
        warnings: usize,
        metadata_warnings: usize,
        limit: bool,
        signature: Option<[u8; 32]>,
    ) -> bool {
        if !self.current(epoch) {
            return false;
        }
        let Ok(mut state) = self.data.lock() else {
            return false;
        };
        if state.status.epoch != epoch
            || state.status.phase != "scanning"
            || state.status.warnings != warnings
            || state.status.metadata_warnings != metadata_warnings
            || !self.current(epoch)
        {
            return false;
        }
        state.status.phase = if limit { "partial" } else { "ready" }.into();
        state.status.error = if limit {
            Some("library_limit")
        } else if warnings > 0 {
            Some("windows_library_check_unconfirmed")
        } else {
            None
        };
        state.signature = signature;
        self.checks.notify();
        true
    }
    fn persist_snapshot_after_scan(
        &self,
        signature: Option<[u8; 32]>,
        warnings: crate::local_diagnostics::LibraryWarningCounts,
        warnings_by_root: &BTreeMap<PathBuf, crate::local_diagnostics::LibraryWarningCounts>,
        structurally_complete: bool,
        added_ok: bool,
    ) {
        let warning_count = warnings.total();
        let Ok(state) = self.data.lock() else {
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::SnapshotSaveSkipped {
                    reason: crate::local_diagnostics::SnapshotSaveSkip::CatalogState,
                    warning_count,
                }, Some("library_state"));
            return;
        };
        let skip = if !structurally_complete || signature.is_none() {
            Some(crate::local_diagnostics::SnapshotSaveSkip::Unqualified)
        } else if !warnings.metadata_only() {
            Some(crate::local_diagnostics::SnapshotSaveSkip::Warnings)
        } else if !added_ok || state.added_error {
            Some(crate::local_diagnostics::SnapshotSaveSkip::AddedState)
        } else if state.status.phase != "scanning"
            || state.status.epoch != self.epoch.load(Ordering::Acquire)
            || state.status.warnings != warning_count
            || state.status.metadata_warnings != warnings.metadata_decode
            || state.status.error.is_some()
            || state.signature.is_some()
            || (warning_count == 0
                && source_scope::signature(&state.roots, &state.root_summaries)
                    != signature)
            || state.roots.is_empty()
        {
            Some(crate::local_diagnostics::SnapshotSaveSkip::CatalogState)
        } else {
            None
        };
        let Some(signature) = signature.filter(|_| skip.is_none()) else {
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::SnapshotSaveSkipped {
                    reason: skip.unwrap_or(crate::local_diagnostics::SnapshotSaveSkip::Unqualified),
                    warning_count,
                }, None);
            return;
        };
        let roots = state.roots.clone();
        let records = state.albums.clone();
        let album_count = records.len();
        let track_count = records.values().map(|record| record.tracks.len()).sum();
        drop(state);
        match snapshot::save_v2(&self.config, &roots, signature, &records, warnings_by_root, warnings) {
            Ok(generation) => {
                let _ = self.set_catalog_generation(generation);
                crate::local_diagnostics::record_library_event(crate::local_diagnostics::LibraryEvent::SnapshotSaved {
                    root_count: roots.len(), album_count, track_count,
                    provisional: warning_count != 0, metadata_warning_count: warnings.metadata_decode,
                }, None)
            }
            Err(reason) => {
                eprintln!("Windows library: catalog_snapshot_write_refused");
                crate::local_diagnostics::record_library_event(
                    crate::local_diagnostics::LibraryEvent::SnapshotSaveRejected {
                        reason: reason.into(),
                    }, Some("windows_catalog_snapshot_invalid"));
            }
        }
    }
    fn persist_snapshot_if_confirmed(&self) {
        let Ok(state) = self.data.lock() else {
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::SnapshotSaveSkipped {
                    reason: crate::local_diagnostics::SnapshotSaveSkip::CatalogState,
                    warning_count: 0,
                }, Some("library_state"));
            return;
        };
        let skip = if state.status.warnings != 0 {
            Some(crate::local_diagnostics::SnapshotSaveSkip::Warnings)
        } else if state.added_error {
            Some(crate::local_diagnostics::SnapshotSaveSkip::AddedState)
        } else if state.signature.is_none() {
            Some(crate::local_diagnostics::SnapshotSaveSkip::Unqualified)
        } else if state.status.phase != "ready" || state.status.error.is_some()
            || source_scope::signature(&state.roots, &state.root_summaries) != state.signature
        {
            Some(crate::local_diagnostics::SnapshotSaveSkip::CatalogState)
        } else {
            None
        };
        let Some(signature) = state.signature.filter(|_| skip.is_none()) else {
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::SnapshotSaveSkipped {
                    reason: skip.unwrap_or(crate::local_diagnostics::SnapshotSaveSkip::Unqualified),
                    warning_count: state.status.warnings,
                }, None);
            return;
        };
        let roots = state.roots.clone();
        let records = state.albums.clone();
        let album_count = records.len();
        let track_count = records.values().map(|record| record.tracks.len()).sum();
        drop(state);
        let warnings = roots.iter().cloned().map(|root| (root, crate::local_diagnostics::LibraryWarningCounts::default())).collect();
        match snapshot::save_v2(&self.config, &roots, signature, &records, &warnings, crate::local_diagnostics::LibraryWarningCounts::default()) {
            Ok(generation) => {
                let _ = self.set_catalog_generation(generation);
                crate::local_diagnostics::record_library_event(crate::local_diagnostics::LibraryEvent::SnapshotSaved {
                    root_count: roots.len(), album_count, track_count,
                    provisional: false, metadata_warning_count: 0,
                }, None)
            }
            Err(reason) => {
                // The atomic writer preserves the previous valid file on failure.
                eprintln!("Windows library: catalog_snapshot_write_refused");
                crate::local_diagnostics::record_library_event(
                    crate::local_diagnostics::LibraryEvent::SnapshotSaveRejected {
                        reason: reason.into(),
                    }, Some("windows_catalog_snapshot_invalid"));
            }
        }
    }
    fn publish(
        &self,
        epoch: u64,
        batch: &mut Vec<Record>,
        warnings: usize,
        metadata_warnings: usize,
        finished: bool,
        limit: bool,
        signature: Option<[u8; 32]>,
    ) -> bool {
        if !self.current(epoch) {
            return false;
        }
        let Ok(mut s) = self.data.lock() else {
            return false;
        };
        if s.status.epoch != epoch || !self.current(epoch) {
            return false;
        }
        self.imports.feed(epoch,batch);
        for mut item in batch.drain(..) {
            let existing = s
                .albums
                .entry(item.album.id.clone())
                .or_insert_with(|| Record {
                    root: item.root.clone(),
                    album: item.album.clone(),
                    tracks: Vec::new(),
                    folder: item.folder.clone(),
                    cover: item.cover.clone(),
                    search: String::new(),
                    durations: HashMap::new(),
                });
            existing.search.push_str(&item.search);
            existing.search.push('\n');
            existing.durations.extend(item.durations);
            existing.tracks.append(&mut item.tracks);
            existing.album.track_count = existing.tracks.len();
            if existing.cover.is_none() {
                existing.cover = item.cover;
                existing.album.has_cover = existing.cover.is_some()
            }
        }
        s.status.revision += 1;
        s.status.albums = s.albums.len();
        s.status.tracks = s.albums.values().map(|a| a.tracks.len()).sum();
        s.status.warnings = warnings;
        s.status.metadata_warnings = metadata_warnings;
        if finished {
            s.status.phase = if limit { "partial" } else { "ready" }.into();
            s.status.error = if limit { Some("library_limit") } else if warnings > 0 { Some("windows_library_check_unconfirmed") } else { None };
            s.signature = signature;
        }
        self.checks.notify();
        true
    }
    #[cfg(test)]
    pub fn albums(&self, query: &str, offset: usize, limit: usize) -> Result<Albums> {
        self.albums_ordered(query, offset, limit, "artist")
    }
    pub fn albums_ordered(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        order: &str,
    ) -> Result<Albums> {
        if query.len() > 2048
            || query.chars().count() > 512
            || limit == 0
            || limit > 512
            || offset > MAX_TRACKS
            || !["artist", "title", "year"].contains(&order)
        {
            return Err("invalid_argument");
        }
        let query = search_text(query);
        let terms: Vec<_> = query.split_whitespace().collect();
        // Copy only public album rows under the lock; sorting/pagination do not
        // delay catalog publication or track selection.
        let (epoch, revision, mut items) = {
            let s = self.data.lock().map_err(|_| "library_state")?;
            let items: Vec<_> = s
                .albums
                .values()
                .filter(|a| terms.iter().all(|term| a.search.contains(term)))
                .map(|a| a.album.clone())
                .collect();
            (s.status.epoch, s.status.revision, items)
        };
        match order {
            "title" => items.sort_by_cached_key(|a| {
                (search_text(&a.title), search_text(&a.artist), a.id.clone())
            }),
            "year" => items.sort_by_cached_key(|a| {
                (
                    a.year.is_none(),
                    std::cmp::Reverse(a.year.unwrap_or(0)),
                    search_text(&a.title),
                    a.id.clone(),
                )
            }),
            _ => items.sort_by_cached_key(|a| {
                (search_text(&a.artist), search_text(&a.title), a.id.clone())
            }),
        }
        let total = items.len();
        Ok(Albums {
            epoch,
            revision,
            total,
            items: items.into_iter().skip(offset).take(limit).collect(),
        })
    }
    pub fn tracks(&self, album: &str) -> Result<Vec<Track>> {
        let s = self.data.lock().map_err(|_| "library_state")?;
        let mut tracks = s.albums.get(album).ok_or("library_changed")?.tracks.clone();
        tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
        Ok(tracks)
    }
    pub fn select_queue(&self, album: &str, track: Option<&str>) -> Result<Selection> {
        let s = self.data.lock().map_err(|_| "library_state")?;
        let mut tracks = s.albums.get(album).ok_or("library_changed")?.tracks.clone();
        tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
        if tracks.is_empty() || tracks.len() > 1000 {
            return Err("queue_limit");
        }
        let start = match track {
            Some(id) => tracks
                .iter()
                .position(|t| t.id == id)
                .ok_or("library_changed")?,
            None => 0,
        };

        Ok(Selection {
            root: tracks.first().and_then(|t| t.media_root.clone()).ok_or("library_empty")?,
            tracks,
            start,
            album_id: Some(album.to_owned()),
        })
    }
    /// Presentation only: a missing favorite is authoritative only after a
    /// complete clean scan, with every configured root still admitted.
    pub fn favorite_cover(&self, album: &str) -> Result<FavoriteCover> {
        self.favorite_cover_with(album,std::time::Duration::from_secs(6),|path,current|canonical_root_read(path,current))
    }
    fn favorite_cover_with(&self,album:&str,timeout:std::time::Duration,canonical:impl Fn(&Path,&dyn Fn()->bool)->Result<PathBuf>+Send+'static)->Result<FavoriteCover>{
        match self.cover(album) {
            Ok(thumb) => Ok(FavoriteCover { exists: true, thumb }),
            Err(error) => {
                let captured=missing::Proof::capture(self).map_err(|_|error)?;
                if self.data.lock().map_err(|_|"library_state")?.albums.contains_key(album){return Err(error);}
                // One aggregate admission/deadline, not up to32 independent
                // per-root waits. The physical worker retains its permit after
                // a caller timeout and checks retirement before every OS step.
                let library=self.clone();
                let deadline=std::time::Instant::now().checked_add(timeout).ok_or(error)?;
                let captured=crate::windows_io::read(timeout,move|job|{
                    let current=||job.current()&&std::time::Instant::now()<deadline;
                    captured.admit_with(&library,&current,&canonical).map_err(|_|error)?;
                    Ok::<_,&'static str>(captured)
                }).map_err(|_|error)??;
                let state = self.data.lock().map_err(|_| "library_state")?;
                if !captured.matches(self,&state)||state.albums.contains_key(album){return Err(error);}
                Ok(FavoriteCover { exists: false, thumb: None })
            }
        }
    }
    /// Already inside one bounded read worker. No nested filesystem admission.
    pub(crate) fn missing_roots_read(&self,epoch:u64,revision:u64,current:&dyn Fn()->bool)->Result<missing::Proof>{
        let captured=missing::Proof::capture_roots(self)?;
        if !captured.version(epoch,revision){return Err("library_changed");}
        captured.admit_with(self,current,&|path,current|canonical_root_read(path,current))?;
        captured.validate(self)?;Ok(captured)
    }
    pub(crate) fn import_artwork(&self,lease:Arc<imports::ArtworkLease>)->Result<Option<String>> {self.import_artwork_with(lease,imports::thumbnail)}
    fn import_artwork_with(&self,lease:Arc<imports::ArtworkLease>,decode:impl FnOnce(&[u8])->Result<Option<String>>+Send+'static)->Result<Option<String>> {
        lease.ensure_current()?;
        let(album,epoch,source)=lease.binding();let selected=CoverSource::capture(self,album)?;
        if selected.epoch!=epoch||source_scope::source_id(&selected.root)!=source{return Err("LOCAL_IMPORT_PREVIEW_STALE");}
        let root=selected.root.clone();let path=selected.cover.clone().ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        let physical=lease.clone();
        let value=crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
            // Retain the actual artwork reservation past caller timeout and all
            // filesystem/codec work; a revoked lease never frees a live slot.
            let _physical=physical;
            if !job.current(){return Err("cover_unavailable");}_physical.ensure_current()?;
            let bytes=external_cover_bytes(&path,&root,512*1024)?;
            if !job.current(){return Err("cover_unavailable");}_physical.ensure_current()?;
            let image=match bytes{Some(bytes)=>decode(&bytes)?,None=>None};
            if !job.current(){return Err("cover_unavailable");}_physical.ensure_current()?;Ok(image)
        }).map_err(|e|if e==crate::windows_io::Error::Busy{"engine_busy"}else{"cover_unavailable"})??;
        selected.validate(self)?;lease.ensure_current()?;Ok(value)
    }
    pub fn cover(&self, album: &str) -> Result<Option<String>> {
        let _read = cover_wait::admit()?;
        let selected = cover_wait::capture(self, album)?;
        let root=selected.root.clone();let cover=selected.cover.clone();let media=selected.media.clone();
        let value=crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
            let _read=_read; // Retain the physical cover permit through OS return.
            if !job.current(){return Err("cover_unavailable");}
            let value=match cover {Some(path)=>external_cover(&path,&root),None=>embedded_cover(&media,&root)};
            if !job.current(){return Err("cover_unavailable");}
            value
        }).map_err(|e|if e==crate::windows_io::Error::Busy{"engine_busy"}else{"cover_unavailable"})??;
        selected.validate(self)?;
        Ok(value)
    }
}


#[derive(Serialize)]
pub struct FavoriteCover { pub exists: bool, pub thumb: Option<String> }

struct CoverSource {
    album: String,
    epoch: u64,
    root: PathBuf,
    cover: Option<PathBuf>,
    media: PathBuf,
}
impl CoverSource {
    fn capture(library: &Library, album: &str) -> Result<Self> {
        let state = library.data.lock().map_err(|_| "library_state")?;
        if !library.current(state.status.epoch) {
            return Err("library_changed");
        }
        let record = state.albums.get(album).ok_or("library_changed")?;
        Ok(Self {
            album: album.into(),
            epoch: state.status.epoch,
            root: record.root.clone(),
            cover: record.cover.clone(),
            media: record.tracks.first().ok_or("library_changed")?.path.clone(),
        })
    }
    fn validate(&self, library: &Library) -> Result<()> {
        let current = Self::capture(library, &self.album)?;
        if self.epoch != current.epoch
            || self.root != current.root
            || self.cover != current.cover
            || self.media != current.media
        {
            return Err("library_changed");
        }
        Ok(())
    }
}
fn cover_read_error(error: crate::local_file_info::InfoError) -> &'static str {
    match error {
        crate::local_file_info::InfoError::Stale
        | crate::local_file_info::InfoError::OutsideConfiguredRoots => "library_changed",
        _ => "cover_unavailable",
    }
}
fn external_cover(path: &Path, root: &Path) -> Result<Option<String>> {
    external_cover_bytes(path,root,4*1024*1024).map(|bytes|bytes.and_then(|bytes|image_uri(&bytes)))
}
fn external_cover_bytes(path:&Path,root:&Path,limit:usize)->Result<Option<Vec<u8>>> {
    let meta = fs::symlink_metadata(path).map_err(|_| "cover_unavailable")?;
    if linked(&meta) || !meta.is_file() || meta.len() > limit as u64 {
        return Ok(None);
    }
    let path = path.canonicalize().map_err(|_| "cover_unavailable")?;
    if !path.starts_with(root) {
        return Err("library_changed");
    }
    crate::windows_file_info::with_bound_file(root, &path, |file, len| {
        if len > limit as u64 {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| crate::local_file_info::InfoError::Unavailable)?;
        if bytes.len() > limit {
            return Ok(None);
        }
        Ok(Some(bytes))
    })
    .map_err(cover_read_error)
}

fn find_cover(folder: &Path, current:&dyn Fn()->bool) -> Option<PathBuf> {
    fs::read_dir(folder)
        .ok()
        .into_iter()
        .flatten()
        .take_while(|_|current())
        .filter_map(|v| v.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let score = [
                "cover.jpg",
                "folder.jpg",
                "front.jpg",
                "cover.png",
                "folder.png",
                "front.png",
            ]
            .iter()
            .position(|v| *v == name)?;
            Some((score, e.path()))
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .next()
}

fn search_text(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            Some(match c {
                'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'í' | 'ì' | 'î' | 'ï' => 'i',
                'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
                'ú' | 'ù' | 'û' | 'ü' => 'u',
                'ç' => 'c',
                'ñ' => 'n',
                '\u{0300}'..='\u{036f}' => return None,
                other => other,
            })
        })
        .collect()
}

fn metadata_record(path: &Path, root: &Path, current:&dyn Fn()->bool) -> (Record, bool) {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let parent = path.parent().unwrap_or(root);
    let folder = if disc_folder(parent).is_some() {
        parent
            .parent()
            .filter(|p| p.starts_with(root))
            .unwrap_or(parent)
    } else {
        parent
    };
    let fallback = folder
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    // Properties may be estimated by the format reader. Never use this
    // presentation metadata as playback timing, source evidence or admission.
    let inspected = crate::windows_file_info::with_bound_file_while(root, path, current, |mut file, _| {
    if !current(){return Err(crate::local_file_info::InfoError::Unavailable);}
    let properties = file.try_clone().ok().and_then(|file| {
        Probe::new(crate::windows_file_info::Budget::new(scan_io::Reader::new(file,current)))
            .options(
                ParseOptions::new()
                    .read_properties(true)
                    .read_cover_art(false),
            )
            .guess_file_type()
            .ok()?
            .read()
            .ok()
    });
    let duration = properties
        .as_ref()
        .map(|file| file.properties().duration().as_secs_f64())
        .filter(|value| value.is_finite() && *value > 0.0);
    // A malformed/unsupported/slow properties block must not discard tags that
    // the previous tags-only reader accepted. Keep that exact fallback policy.
    let tagged = properties.or_else(|| {
        if !current(){return None;}
        file.seek(SeekFrom::Start(0)).ok()?;
        file.try_clone().ok().and_then(|file| {
            Probe::new(Budget {
                file:scan_io::Reader::new(file,current),
                remaining: 8 * 1024 * 1024,
            })
            .options(
                ParseOptions::new()
                    .read_properties(false)
                    .read_cover_art(false),
            )
            .guess_file_type()
            .ok()?
            .read()
            .ok()
        })
    });
        Ok((tagged, duration))
    }).ok();
    let (tagged, duration) = inspected.unwrap_or((None, None));
    let tag = tagged
        .as_ref()
        .and_then(|f| f.primary_tag().or_else(|| f.first_tag()));
    let title = tag
        .and_then(|t| t.title())
        .map(|v| v.chars().take(512).collect::<String>())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let album = tag
        .and_then(|t| t.album())
        .map(|v| v.chars().take(512).collect::<String>())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(fallback);
    let artist = tag
        .and_then(|t| {
            t.get_string(ItemKey::AlbumArtist)
                .map(|v| v.chars().take(512).collect::<String>())
                .or_else(|| t.artist().map(|v| v.chars().take(512).collect::<String>()))
        })
        .unwrap_or_default();
    let track_artist = tag
        .and_then(|t| t.artist())
        .map(|v| v.chars().take(512).collect::<String>())
        .unwrap_or_else(|| artist.clone());
    let genre = tag
        .and_then(|t| t.genre())
        .map(|v| v.chars().take(512).collect::<String>());
    let year = tag.and_then(|t| {
        t.get_string(ItemKey::RecordingDate)
            .and_then(|v| v.get(..4))
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0 && *v <= 9999)
    });
    let disc = tag
        .and_then(|t| t.disk())
        .or_else(|| disc_folder(parent))
        .unwrap_or(1);
    let number = tag.and_then(|t| t.track()).unwrap_or(0);
    let key = id(&format!("{}\0{album}", folder.to_string_lossy()));
    let track_id = id(&path.to_string_lossy());
    let cover = None;
    let durations = duration
        .into_iter()
        .map(|value| (track_id.clone(), value))
        .collect();
    let search = search_text(&format!(
        "{album} {artist} {} {title} {track_artist}",
        genre.as_deref().unwrap_or("")
    ));
    (
        Record {
            root: root.to_owned(),
            album: Album {
                id: key.clone(),
                title: album.clone(),
                artist: artist.clone(),
                year,
                genre,
                track_count: 1,
                has_cover: cover.is_some(),
                added_at: None,
            },
            tracks: vec![Track {
                id: track_id,
                catalog_album_id: Some(key),
                title,
                artist: track_artist,
                album,
                disc,
                number,
                file_name,
                path: path.to_owned(),
                media_root: Some(root.to_owned()),
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
            }],
            folder: folder.to_owned(),
            cover,
            search,
            durations,
        },
        tagged.is_none(),
    )
}

fn save(path: &Path, value: &Saved) -> Result<()> {save_mode(path,value,true)}
fn save_mode(path: &Path, value: &Saved, replace:bool) -> Result<()> {
    let parent = path.parent().ok_or("library_settings")?;
    fs::create_dir_all(parent).map_err(|_| "library_settings")?;
    let metadata=fs::symlink_metadata(parent).map_err(|_|"library_settings")?;
    if !metadata.is_dir()||linked(&metadata){return Err("library_settings");}
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| "random_failed")?;
    let temp = parent.join(format!("folder-{}.tmp", id(&format!("{random:?}"))));
    let outcome = (|| -> Result<()> {
        use std::io::Write;
        let bytes = serde_json::to_vec(value).map_err(|_| "library_settings")?;
        if bytes.len() > 1024 * 1024 + 16384 { return Err("library_settings"); }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| "library_settings")?;
        file.write_all(&bytes).map_err(|_| "library_settings")?;
        file.sync_all().map_err(|_| "library_settings")?;
        drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let from: Vec<_> = temp.as_os_str().encode_wide().chain([0]).collect();
            let to: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    if replace{MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH}else{MOVEFILE_WRITE_THROUGH},
                )
            } == 0
            {
                return Err("library_settings");
            }
        }
        #[cfg(not(windows))]
        if replace{fs::rename(&temp,path).map_err(|_|"library_settings")?;}else{fs::hard_link(&temp,path).map_err(|_|"library_settings")?;}
        Ok(())
    })();
    if temp.exists() && fs::remove_file(&temp).is_err() {
        return Err("library_settings");
    }
    outcome
}

fn image_uri(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mime = if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else {
        return None;
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn embedded_cover(path: &Path, root: &Path) -> Result<Option<String>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "cover_unavailable")?;
    if linked(&metadata) || !metadata.is_file() {
        return Ok(None);
    }
    let path = path.canonicalize().map_err(|_| "cover_unavailable")?;
    if !path.starts_with(root) {
        return Err("library_changed");
    }
    crate::windows_file_info::with_bound_file(root, &path, |file, _| {
        let tagged = Probe::new(Budget {
            file,
            remaining: 8 * 1024 * 1024,
        })
        .options(
            ParseOptions::new()
                .read_properties(false)
                .read_cover_art(true),
        )
        .guess_file_type()
        .ok()
        .and_then(|p| p.read().ok());
        Ok(tagged.as_ref().and_then(|f| {
            f.tags()
                .iter()
                .flat_map(|t| t.pictures())
                .find_map(|picture| image_uri(picture.data()))
        }))
    })
    .map_err(cover_read_error)
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;

// Original frontend models projected from the same catalog, without another index.
pub fn track_uri(track: &Track) -> String {
    if let Some(upnp)=&track.upnp{return format!("upnp/{}",upnp.session_id);}
    match &track.catalog_album_id {
        Some(album) => format!("windows/{album}/{}/{}", track.disc, track.id),
        None => format!("windows-file/{}", track.id),
    }
}
fn item(track: &Track, durations: &HashMap<String, f64>) -> serde_json::Value {
    let mut value = serde_json::json!({"item_type":"file","path":track_uri(track),"name":track.file_name,"title":track.title,"artist":track.artist,"album":track.album});
    if let Some(duration) = durations.get(&track.id) {
        value["duration"] = serde_json::json!(duration);
    }
    value
}
impl Library {
    // Complete immutable presentation snapshot only; no filesystem/MPD I/O.
    pub fn complete_album_snapshot(&self) -> Result<(String, Vec<Album>)> {
        let s = self.data.lock().map_err(|_| "library_state")?;
        let token = self.snapshot_token(&s)?;
        if s.added_error { return Err("library_added_unavailable"); }
        Ok((
            token,
            s.albums
                .values()
                .map(|record| record.album.clone())
                .collect(),
        ))
    }
    fn snapshot_token(&self, s: &State) -> Result<String> {
        if !self.current(s.status.epoch)
            || s.status.phase != "ready"
            || s.status.error.is_some()
            || !s.sources.local
            || s.signature.is_none()
            || s.cached_signature.is_some()
            || s.root.is_none()
        {
            return Err("library_changed");
        }
        Ok(format!("{}:{}", s.status.epoch, s.status.revision))
    }
    pub(crate) fn metadata_snapshot_token(&self)->Result<String>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if !state.sources.local{return Err("local_source_disabled");}self.snapshot_token(&state)
    }
    pub(crate) fn check_metadata_snapshot(&self,token:&str)->Result<()>{
        if self.metadata_snapshot_token()?!=token{return Err("library_changed");}Ok(())
    }
    pub(crate) fn metadata_members(&self,album:&str,token:&str)->Result<Option<Vec<Track>>>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if !state.sources.local||self.snapshot_token(&state)?!=token{return Err("library_changed");}
        let Some(record)=state.albums.get(album)else{return Ok(None);};
        if record.tracks.is_empty()||record.tracks.len()>500{return Err("metadata_limit");}Ok(Some(record.tracks.clone()))
    }
    pub fn album_snapshot_token(&self) -> Result<String> {
        let state = self.data.lock().map_err(|_| "library_state")?;
        self.snapshot_token(&state)
    }
    pub fn check_album_snapshot(&self, token: &str) -> Result<()> {
        if self.album_snapshot_token()? != token {
            return Err("library_changed");
        }
        Ok(())
    }
    pub fn configured_root(&self) -> Result<Option<PathBuf>> {
        Ok(self.data.lock().map_err(|_| "library_state")?.root.clone())
    }
    /// Only app setup calls this, before any account/store is initialized. A
    /// fresh profile gets an explicit pending record; interrupted wizard/PIN
    /// setup must still reopen the wizard. Old profiles are never reset.
    pub fn prepare_first_run(&self,existing_profile:bool)->Result<()>{
        let _change=self.change.lock().map_err(|_|"library_state")?;
        if self.setup_initialized.load(Ordering::Acquire){return Ok(());}
        if existing_profile {
            self.data.lock().map_err(|_|"library_state")?.first_run=false;
        }else{
            let value=Saved{version:3,usage_mode:UsageMode::Full,online_artwork_enabled:false,experimental_features:false,first_run:Some(true),root:None,roots:Vec::new(),sources:Some(Sources::default()),catalog_rebuild_attempted:false,catalog_generation:None};
            save_mode(&self.config,&value,false)?;
            self.remember_settings(&value)?;
        }
        self.setup_initialized.store(true,Ordering::Release);Ok(())
    }
    fn check_current_settings(&self)->Result<()>{
        let metadata=match fs::symlink_metadata(&self.config){
            Ok(m)=>m,
            Err(e)if e.kind()==std::io::ErrorKind::NotFound&&!self.settings_current.load(Ordering::Acquire)=>return Ok(()),
            Err(_)=>return Err("library_settings"),
        };
        if !metadata.is_file()||linked(&metadata)||metadata.len()>1024*1024+16384{return Err("library_settings");}
        let parent=fs::symlink_metadata(self.config.parent().ok_or("library_settings")?).map_err(|_|"library_settings")?;
        if !parent.is_dir()||linked(&parent){return Err("library_settings");}
        let mut bytes=Vec::new();File::open(&self.config).map_err(|_|"library_settings")?.take(1024*1024+16385).read_to_end(&mut bytes).map_err(|_|"library_settings")?;
        if bytes.len()>1024*1024+16384{return Err("library_settings");}
        let value:Saved=serde_json::from_slice(&bytes).map_err(|_|"library_settings")?;
        value.sources.unwrap_or_default().validate()?;value.folders()?;
        let fingerprint: [u8;32]=Sha256::digest(&bytes).into();
        if self.settings_fingerprint.lock().map_err(|_|"library_state")?.as_ref()!=Some(&fingerprint){return Err("library_settings");}
        Ok(())
    }
    fn remember_settings(&self,value:&Saved)->Result<()>{
        let bytes=serde_json::to_vec(value).map_err(|_|"library_settings")?;
        *self.settings_fingerprint.lock().map_err(|_|"library_state")?=Some(Sha256::digest(&bytes).into());
        self.settings_current.store(true,Ordering::Release);Ok(())
    }
    fn store_settings(&self,value:&Saved)->Result<()>{
        // The fingerprint advances on our own durable write, even if a later
        // catalog/host step fails. Rollback can restore that exact publication;
        // unrelated/corrupt/deleted/linked bytes are never treated as repair.
        self.check_current_settings()?;save(&self.config,value)?;self.remember_settings(value)
    }
    fn set_catalog_rebuild_attempted(&self, attempted: bool) -> Result<()> {
        self.check_current_settings()?;
        let mut bytes = Vec::new();
        File::open(&self.config).map_err(|_| "library_settings")?
            .take(1024 * 1024 + 16385).read_to_end(&mut bytes).map_err(|_| "library_settings")?;
        if bytes.len() > 1024 * 1024 + 16384 { return Err("library_settings"); }
        let mut saved: Saved = serde_json::from_slice(&bytes).map_err(|_| "library_settings")?;
        saved.catalog_rebuild_attempted = attempted;
        self.store_settings(&saved)?;
        self.data.lock().map_err(|_| "library_state")?.catalog_rebuild_attempted = attempted;
        Ok(())
    }
    fn set_catalog_generation(&self, generation: u64) -> Result<()> {
        self.check_current_settings()?;
        let mut bytes = Vec::new();
        File::open(&self.config).map_err(|_| "library_settings")?
            .take(1024 * 1024 + 16385).read_to_end(&mut bytes).map_err(|_| "library_settings")?;
        if bytes.len() > 1024 * 1024 + 16384 { return Err("library_settings"); }
        let mut saved: Saved = serde_json::from_slice(&bytes).map_err(|_| "library_settings")?;
        saved.catalog_rebuild_attempted = false;
        saved.catalog_generation = Some(generation);
        self.store_settings(&saved)?;
        let mut state = self.data.lock().map_err(|_| "library_state")?;
        state.catalog_rebuild_attempted = false;
        state.catalog_generation = Some(generation);
        Ok(())
    }
    pub fn first_run(&self)->Result<bool>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.status.error==Some("library_settings"){return Err("library_settings");}
        Ok(state.first_run)
    }
    /// Last durable setup step, after the existing sources/audio transaction
    /// has succeeded. This never starts an engine or waits for catalog scans.
    pub fn complete_first_run(&self)->Result<()>{
        let _change=self.change.lock().map_err(|_|"library_state")?;
        let (roots,sources,experimental,online,usage_mode)={let state=self.data.lock().map_err(|_|"library_state")?;
            if state.status.error==Some("library_settings"){return Err("library_settings");}
            if !state.first_run{return Ok(());}
            (state.roots.clone(),state.sources,state.experimental_features,state.online_artwork_enabled,state.usage_mode)};
        // A concurrent/corrupt/linked current record is authoritative. Do not
        // recreate an absent record or overwrite a changed record as repair.
        let parent=self.config.parent().ok_or("library_settings")?;
        let parent_meta=fs::symlink_metadata(parent).map_err(|_|"library_settings")?;
        if !parent_meta.is_dir()||linked(&parent_meta){return Err("library_settings");}
        let m=fs::symlink_metadata(&self.config).map_err(|_|"library_settings")?;
        if !m.is_file()||linked(&m)||m.len()>1024*1024+16384{return Err("library_settings");}
        let mut bytes=Vec::new();File::open(&self.config).map_err(|_|"library_settings")?
            .take(1024*1024+16385).read_to_end(&mut bytes).map_err(|_|"library_settings")?;
        if bytes.len()>1024*1024+16384{return Err("library_settings");}
        let current:Saved=serde_json::from_slice(&bytes).map_err(|_|"library_settings")?;
        if current.version!=3||current.first_run!=Some(true)||current.root.is_some()||current.roots!=roots||current.sources!=Some(sources)||current.experimental_features!=experimental||current.online_artwork_enabled!=online||current.usage_mode!=usage_mode{return Err("library_settings");}
        let (catalog_rebuild_attempted,catalog_generation)={let state=self.data.lock().map_err(|_|"library_state")?;(state.catalog_rebuild_attempted,state.catalog_generation)};
        self.store_settings(&Saved{version:3,usage_mode,online_artwork_enabled:online,experimental_features:experimental,first_run:Some(false),root:None,roots,sources:Some(sources),catalog_rebuild_attempted,catalog_generation})?;
        self.data.lock().map_err(|_|"library_state")?.first_run=false;Ok(())
    }
    pub(crate) fn usage_mode(&self)->Result<UsageMode>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.status.error==Some("library_settings"){return Err("library_settings");}Ok(state.usage_mode)
    }
    pub(crate) fn online_artwork_enabled(&self)->Result<bool>{
        let state=self.data.lock().map_err(|_|"library_state")?;if state.status.error==Some("library_settings"){return Err("library_settings");}Ok(state.online_artwork_enabled)
    }
    pub(crate) fn online_artwork_context(&self,album:&str)->Result<(String,u64,bool,Album)>{
        let _change=self.change.try_lock().map_err(|_|"ARTWORK_CONTEXT_CHANGED")?;
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.first_run||!state.sources.local||self.stopping.load(Ordering::Acquire){return Err("ARTWORK_CONTEXT_CHANGED");}
        let token=self.snapshot_token(&state)?;let record=state.albums.get(album).ok_or("ARTWORK_CONTEXT_CHANGED")?;
        Ok((token,state.editions_revision,state.online_artwork_enabled,record.album.clone()))
    }
    pub(crate) fn check_online_artwork_context(&self,token:&str,revision:u64)->Result<()>{
        let _change=self.change.try_lock().map_err(|_|"ARTWORK_CONTEXT_CHANGED")?;
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.first_run||!state.sources.local||self.stopping.load(Ordering::Acquire)||state.editions_revision!=revision||self.snapshot_token(&state)?!=token{return Err("ARTWORK_CONTEXT_CHANGED");}Ok(())
    }
    pub(crate) fn experimental_features(&self)->Result<bool>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.status.error==Some("library_settings"){return Err("library_settings");}Ok(state.experimental_features)
    }
    // A generation, not a source capability: disable/re-enable and rollback
    // cannot revive a read admitted under a previous settings publication.
    pub(crate) fn edition_scope(&self)->Result<(u64,Sources)>{
        let _change=self.change.try_lock().map_err(|_|"EDITIONS_SOURCE")?;
        let state=self.data.lock().map_err(|_|"library_state")?;
        if self.stopping.load(Ordering::Acquire)||state.status.error==Some("library_settings")||state.first_run||state.usage_mode!=UsageMode::Full||!state.experimental_features{return Err("EDITIONS_DISABLED");}
        Ok((state.editions_revision,state.sources))
    }
    pub fn preferences(&self)->Result<(Vec<PathBuf>,Sources)>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.status.error==Some("library_settings"){return Err("library_settings");}
        Ok((state.roots.clone(),state.sources))
    }
    // Optional snapshot consumers must never wait for catalog/config locks.
    pub(crate) fn observed_usage_mode(&self)->Option<UsageMode>{
        if self.stopping.load(Ordering::Acquire){return None;}
        let state=self.data.try_lock().ok()?;
        (state.status.error!=Some("library_settings")).then_some(state.usage_mode)
    }
    // Lifecycle workers may wait for this short catalog lock. Effect guards use
    // the nonblocking observation below and never wait under the audio actor.
    pub(crate) fn network_scope(&self)->Result<(bool,bool)>{
        if self.stopping.load(Ordering::Acquire){return Err("shutting_down");}
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.status.error==Some("library_settings"){return Err("library_settings");}
        Ok((state.usage_mode==UsageMode::Full&&!state.first_run,state.experimental_features))
    }
    pub(crate) fn observed_network_scope(&self)->Option<(bool,bool)>{
        if self.stopping.load(Ordering::Acquire){return None;}
        let state=self.data.try_lock().ok()?;
        (state.status.error!=Some("library_settings")).then_some((state.usage_mode==UsageMode::Full&&!state.first_run,state.experimental_features))
    }
    pub(crate) fn observed_sources(&self)->Option<Sources>{
        let state=self.data.try_lock().ok()?;
        (state.status.error!=Some("library_settings")).then_some(state.sources)
    }
    pub fn sources(&self)->Result<Sources>{
        let state=self.data.lock().map_err(|_|"library_state")?;
        if state.status.error==Some("library_settings"){return Err("library_settings");}
        Ok(state.sources)
    }
    // Flags and folders share one native atomic record. A flags-only change
    // does not reopen offline local roots, rescan or replace their catalog.
    pub fn configure(&self, roots:Option<Vec<PathBuf>>, sources:Sources)->Result<Status>{self.configure_options(roots,sources,None)}
    pub(crate) fn configure_options(&self,roots:Option<Vec<PathBuf>>,sources:Sources,experimental:Option<bool>)->Result<Status>{self.configure_features(roots,sources,experimental,None)}
    pub(crate) fn configure_features(&self,roots:Option<Vec<PathBuf>>,sources:Sources,experimental:Option<bool>,online:Option<bool>)->Result<Status>{self.configure_preferences(roots,sources,experimental,online,None)}
    pub(crate) fn configure_preferences(&self,roots:Option<Vec<PathBuf>>,sources:Sources,experimental:Option<bool>,online:Option<bool>,mode:Option<UsageMode>)->Result<Status>{
        self.configure_import_preferences(roots,sources,experimental,online,mode,None)
    }
    pub(crate) fn configure_import_preferences(&self,roots:Option<Vec<PathBuf>>,sources:Sources,experimental:Option<bool>,online:Option<bool>,mode:Option<UsageMode>,owner:Option<&imports::Guard>)->Result<Status>{
        sources.validate()?;
        let _change=self.change.lock().map_err(|_|"library_state")?;
        if self.stopping.load(Ordering::Acquire){return Err("shutting_down");}
        let experimental=experimental.unwrap_or(self.experimental_features()?);let online=online.unwrap_or(self.online_artwork_enabled()?);let usage_mode=mode.unwrap_or(self.usage_mode()?);
        if let Some(roots)=roots {
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::ScanStarted {
                    origin: crate::local_diagnostics::LibraryScanOrigin::RootConfiguration,
                    root_count: roots.len(),
                }, None);
            return self.begin_locked_import_preferences(roots,true,Some(sources),Some(experimental),Some(online),Some(usage_mode),owner);
        }
        let (roots,before)={let state=self.data.lock().map_err(|_|"library_state")?;if state.status.error==Some("library_settings"){return Err("library_settings");}(state.roots.clone(),state.sources)};
        let (catalog_rebuild_attempted,catalog_generation)={let state=self.data.lock().map_err(|_|"library_state")?;(state.catalog_rebuild_attempted,state.catalog_generation)};
        self.store_settings(&Saved{version:3,usage_mode,online_artwork_enabled:online,experimental_features:experimental,first_run:Some(self.first_run()?),root:None,roots:roots.clone(),sources:Some(sources),catalog_rebuild_attempted,catalog_generation})?;
        if sources.local && !before.local {
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::ScanStarted {
                    origin: crate::local_diagnostics::LibraryScanOrigin::SourceReenabled,
                    root_count: roots.len(),
                }, None);
            return self.begin_locked_import_preferences(roots,false,Some(sources),Some(experimental),Some(online),Some(usage_mode),owner);
        }
        {let mut state=self.data.lock().map_err(|_|"library_state")?;state.sources=sources;state.experimental_features=experimental;state.online_artwork_enabled=online;state.usage_mode=usage_mode;state.editions_revision=state.editions_revision.wrapping_add(1);
        if !sources.local && before.local {let epoch=self.epoch.fetch_add(1,Ordering::AcqRel)+1;self.imports.supersede(epoch);state.status.epoch=epoch;state.status.phase="ready".into();self.checks.notify();}}
        self.status()
    }
    pub fn configured_roots(&self) -> Result<Vec<PathBuf>> {
        Ok(self.data.lock().map_err(|_| "library_state")?.roots.clone())
    }
    pub fn compatibility_catalog(&self) -> Result<serde_json::Value> {
        let s = self.data.lock().map_err(|_| "library_state")?;
        let source_labels: BTreeMap<_,_> = s.roots.iter().map(|root| (root.clone(), source_scope::source_label(root,&s.roots))).collect();
        let mut rows = Vec::new();
        for record in s.albums.values() {
            let a = &record.album;
            let mut discs: BTreeMap<u32, usize> = BTreeMap::new();
            for t in &record.tracks {
                *discs.entry(t.disc).or_default() += 1;
            }
            rows.push(serde_json::json!({"id":a.id,"title":a.title,"artist":a.artist,"year":a.year.map(|v|v.to_string()),"genres":a.genre.iter().collect::<Vec<_>>(),"added_at":a.added_at.and_then(added::utc),"folder_path":format!("windows/{}",a.id),"source_id":source_scope::source_id(&record.root),"source_label":source_labels.get(&record.root),"track_count":a.track_count,"discs":discs.into_iter().map(|(number,count)|serde_json::json!({"number":number,"label":format!("CD {number}"),"folder_path":format!("windows/{}/{number}",a.id),"track_count":count})).collect::<Vec<_>>()}));
        }
        rows.sort_by_cached_key(|v| {
            (
                v["artist"].as_str().unwrap_or("").to_lowercase(),
                v["title"].as_str().unwrap_or("").to_lowercase(),
                v["id"].as_str().unwrap_or("").to_owned(),
            )
        });
        Ok(serde_json::json!({"epoch":s.status.epoch,"revision":s.status.revision,"albums":rows}))
    }
    pub fn compatibility_tracks(&self, album: &str) -> Result<serde_json::Value> {
        let (mut tracks, durations) = {
            let state = self.data.lock().map_err(|_| "library_state")?;
            let record = state.albums.get(album).ok_or("library_changed")?;
            (record.tracks.clone(), record.durations.clone())
        };
        tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
        let mut discs: BTreeMap<u32, Vec<serde_json::Value>> = BTreeMap::new();
        for track in tracks {
            discs.entry(track.disc).or_default().push(item(&track, &durations));
        }
        Ok(serde_json::json!(discs.into_iter().map(|(number,tracks)|serde_json::json!({"disc":{"number":number,"label":format!("CD {number}"),"folder_path":format!("windows/{album}/{number}"),"track_count":tracks.len()},"tracks":tracks})).collect::<Vec<_>>()))
    }
    pub fn compatibility_search(&self, query: &str) -> Result<serde_json::Value> {
        if query.chars().count() > 512 {
            return Err("invalid_argument");
        }
        let query = search_text(query);
        let terms: Vec<_> = query.split_whitespace().collect();
        if terms.is_empty() {
            return Ok(serde_json::json!({"tracks":[],"limited":false}));
        }
        let s = self.data.lock().map_err(|_| "library_state")?;
        let mut found: Vec<_> = s
            .albums
            .values()
            .flat_map(|r| r.tracks.iter().map(move |t| (t, &r.durations)))
            .filter(|(t, _)| {
                let text = search_text(&format!("{} {} {}", t.title, t.artist, t.album));
                terms.iter().all(|q| text.contains(q))
            })
            .collect();
        found.sort_by_cached_key(|(t, _)| (t.title.to_lowercase(), t.id.clone()));
        Ok(
            serde_json::json!({"limited":found.len()>200,"tracks":found.into_iter().take(200).map(|(track, durations)| item(track, durations)).collect::<Vec<_>>()}),
        )
    }
    pub fn compatibility_selection(&self, uris: &[String], start: usize) -> Result<Selection> {
        if uris.is_empty() || uris.len() > 1000 || start >= uris.len() {
            return Err("queue_limit");
        }
        let s = self.data.lock().map_err(|_| "library_state")?;
        if s.roots.is_empty() { return Err("library_empty"); }
        let mut tracks = Vec::new();
        for uri in uris {
            let parts: Vec<_> = uri.split('/').collect();
            if parts.len() != 4 || parts[0] != "windows" {
                return Err("library_changed");
            }
            let album = s.albums.get(parts[1]).ok_or("library_changed")?;
            let track = album
                .tracks
                .iter()
                .find(|t| t.id == parts[3] && t.disc.to_string() == parts[2])
                .ok_or("library_changed")?;
            tracks.push(track.clone());
        }
        let album_id = tracks
            .first()
            .and_then(|t| t.catalog_album_id.clone())
            .filter(|id| {
                tracks
                    .iter()
                    .all(|t| t.catalog_album_id.as_ref() == Some(id))
            });
        Ok(Selection {
            root: tracks.first().and_then(|t| t.media_root.clone()).ok_or("library_empty")?,
            tracks,
            start,
            album_id,
        })
    }
}

impl Library {
    pub fn compatibility_directory(&self, path: &str) -> Result<serde_json::Value> {
        let s = self.data.lock().map_err(|_| "library_state")?;
        if path.is_empty() {
            return Ok(serde_json::json!(if s.root.is_some() {
                vec![
                    serde_json::json!({"item_type":"directory","path":"windows","name":s.status.root_name}),
                ]
            } else {
                vec![]
            }));
        }
        if path == "windows" {
            let mut rows:Vec<_>=s.albums.values().map(|r|serde_json::json!({"item_type":"directory","path":format!("windows/{}",r.album.id),"name":r.album.title})).collect();
            rows.sort_by_cached_key(|r| r["name"].as_str().unwrap_or("").to_lowercase());
            return Ok(serde_json::json!(rows));
        }
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() < 2 || parts.len() > 3 || parts[0] != "windows" {
            return Err("library_changed");
        }
        let record = s.albums.get(parts[1]).ok_or("library_changed")?;
        let mut tracks = record.tracks.clone();
        tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
        if parts.len() == 3 {
            let disc = parts[2].parse::<u32>().map_err(|_| "library_changed")?;
            return Ok(serde_json::json!(tracks
                .iter()
                .filter(|t| t.disc == disc)
                .map(|track| item(track, &record.durations))
                .collect::<Vec<_>>()));
        }
        let mut discs = std::collections::BTreeSet::new();
        for track in &tracks {
            discs.insert(track.disc);
        }
        if discs.len() > 1 {
            return Ok(serde_json::json!(discs.into_iter().map(|d|serde_json::json!({"item_type":"directory","path":format!("windows/{}/{d}",parts[1]),"name":format!("CD {d}")})).collect::<Vec<_>>()));
        }
        Ok(serde_json::json!(tracks
            .iter()
            .map(|track| item(track, &record.durations))
            .collect::<Vec<_>>()))
    }
}

#[path = "library_check.rs"]
mod check;

#[cfg(test)]
#[path="library_cover_tests.rs"]
mod cover_boundary_tests;

#[cfg(test)]
#[path="multi_roots_tests.rs"]
mod multi_roots_tests;

#[path = "library_cover_wait.rs"]
mod cover_wait;

#[cfg(test)]
#[path="library_source_tests.rs"]
mod source_integration_tests;

#[path = "library_added.rs"]
mod added;

#[cfg(test)]
#[path="library_setup_tests.rs"]
mod setup_tests;

#[cfg(test)]
#[path="library_usage_mode_tests.rs"]
mod usage_mode_tests;

#[path="library_scope.rs"]
mod source_scope;

#[path="library_presentation.rs"]
mod presentation;
pub(crate) use presentation::duration as presentation_duration;
