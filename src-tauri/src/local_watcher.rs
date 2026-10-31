use crate::local_changes::{Batch, Context, MAX_DIRTY_PATHS};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex, RwLock,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const WATCH_MASK: u32 = libc::IN_CLOSE_WRITE
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_DELETE_SELF
    | libc::IN_IGNORED
    | libc::IN_MODIFY
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_MOVE_SELF
    | libc::IN_Q_OVERFLOW
    | libc::IN_UNMOUNT;
const WATCH_DEBOUNCE: Duration = Duration::from_millis(1_200);
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(200);
const BUSY_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const UNCLOSED_WRITE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const UNCLOSED_WRITE_TIMEOUT: Duration = Duration::from_secs(60);

struct WatcherControl {
    stop: AtomicBool,
    registration_gate: RwLock<()>,
}

impl WatcherControl {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            registration_gate: RwLock::new(()),
        }
    }

    fn stopped(&self, shutdown: &AtomicBool) -> bool {
        self.stop.load(Ordering::SeqCst) || shutdown.load(Ordering::SeqCst)
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _registration = self
            .registration_gate
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

#[derive(Clone)]
struct SourceIdentity {
    configured_path: PathBuf,
    physical_path: PathBuf,
    device: u64,
    inode: u64,
}

impl SourceIdentity {
    fn capture(configured_path: PathBuf) -> Result<Self, String> {
        let physical_path = fs::canonicalize(&configured_path).map_err(|error| {
            format!("Fonte local indisponível ao iniciar monitoramento: {error}")
        })?;
        let metadata = fs::metadata(&physical_path)
            .map_err(|error| format!("Falha ao examinar fonte local monitorada: {error}"))?;
        if !metadata.is_dir() {
            return Err("Fonte local monitorada não é um diretório.".to_string());
        }
        Ok(Self {
            configured_path,
            physical_path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn is_available(&self) -> bool {
        let Ok(current_path) = fs::canonicalize(&self.configured_path) else {
            return false;
        };
        if current_path != self.physical_path {
            return false;
        }
        fs::metadata(&current_path).is_ok_and(|metadata| {
            metadata.is_dir() && metadata.dev() == self.device && metadata.ino() == self.inode
        })
    }
}

struct InotifyReader {
    fd: File,
    directories: HashMap<i32, PathBuf>,
}

impl InotifyReader {
    fn new(
        sources: &[SourceIdentity],
        control: &WatcherControl,
        shutdown: &AtomicBool,
    ) -> Result<Option<Self>, String> {
        if control.stopped(shutdown) {
            return Ok(None);
        }
        let raw_fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if raw_fd < 0 {
            return Err(format!(
                "Falha ao inicializar inotify: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut reader = Self {
            fd: unsafe { File::from_raw_fd(raw_fd) },
            directories: HashMap::new(),
        };
        let mut unique_roots = HashSet::new();
        for source in sources {
            if unique_roots.insert(source.physical_path.clone()) {
                if !reader.watch_tree(&source.physical_path, control, shutdown)? {
                    return Ok(None);
                }
            }
        }
        Ok(Some(reader))
    }

    fn watch_tree(
        &mut self,
        root: &Path,
        control: &WatcherControl,
        shutdown: &AtomicBool,
    ) -> Result<bool, String> {
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            if control.stopped(shutdown) {
                return Ok(false);
            }
            if !self.watch_directory(&directory, control, shutdown)? {
                return Ok(false);
            }
            let entries = fs::read_dir(&directory).map_err(|error| {
                format!("Falha ao percorrer fonte local para monitoramento: {error}")
            })?;
            for entry in entries {
                if control.stopped(shutdown) {
                    return Ok(false);
                }
                let entry = entry.map_err(|error| {
                    format!("Falha ao ler entrada da fonte local monitorada: {error}")
                })?;
                let file_type = entry.file_type().map_err(|error| {
                    format!("Falha ao identificar entrada da fonte monitorada: {error}")
                })?;
                if file_type.is_dir() && !file_type.is_symlink() {
                    pending.push(entry.path());
                }
            }
        }
        Ok(true)
    }

    // No physical tree walk on a confirmed source removal. Keep existing
    // registrations for survivors and retire descriptors belonging only to
    // removed roots. Pending paths are fenced/rebased by the owning worker.
    fn retain_roots(&mut self, roots: &[PathBuf]) -> Result<(), String> {
        let retired=self.directories.iter().filter(|(_,path)| !roots.iter().any(|root|path.starts_with(root)))
            .map(|(wd,_)|*wd).collect::<Vec<_>>();
        for wd in retired {
            if unsafe {libc::inotify_rm_watch(self.fd.as_raw_fd(),wd)} < 0
                && std::io::Error::last_os_error().raw_os_error()!=Some(libc::EINVAL) {
                return Err("LOCAL_WATCH_RETIRE_UNCONFIRMED".into());
            }
            self.directories.remove(&wd);
        }
        Ok(())
    }

    fn watch_directory(
        &mut self,
        directory: &Path,
        control: &WatcherControl,
        shutdown: &AtomicBool,
    ) -> Result<bool, String> {
        let _registration = control
            .registration_gate
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if control.stopped(shutdown) {
            return Ok(false);
        }
        let path = CString::new(directory.as_os_str().as_bytes())
            .map_err(|_| "Path local contém NUL e não pode ser monitorado.".to_string())?;
        let descriptor =
            unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), path.as_ptr(), WATCH_MASK) };
        if descriptor < 0 {
            return Err(format!(
                "Falha ao adicionar diretório ao inotify: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.directories.insert(descriptor, directory.to_path_buf());
        Ok(true)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl FileStamp {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
}

struct PendingWrite {
    first_event: Instant,
    last_check: Instant,
    observed: Option<FileStamp>,
    stable_checks: usize,
}

#[derive(Default)]
struct PendingChanges {
    dirty: bool,
    overflowed: bool,
    writing: HashMap<PathBuf, PendingWrite>,
    moved_writes: HashMap<u32, (Instant, Option<PendingWrite>)>,
    last_activity: Option<Instant>,
    paths: BTreeSet<PathBuf>,
    retry_not_before: Option<Instant>,
}

impl PendingChanges {
    fn mark_path(&mut self, path: PathBuf, now: Instant) {
        if self.paths.len() >= MAX_DIRTY_PATHS && !self.paths.contains(&path) {
            self.overflowed = true;
        } else {
            self.paths.insert(path);
        }
        self.dirty = true;
        self.touch(now);
    }

    fn rebase(&mut self, previous: &Context, current: &Context) {
        self.paths.retain(|p| current.retains(previous, p));
        self.writing.retain(|p, _| current.retains(previous, p));
        self.moved_writes.clear();
        self.dirty = !self.paths.is_empty();
        self.retry_not_before = None;
    }

    fn touch(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    fn ready(&self, now: Instant, debounce: Duration) -> bool {
        self.dirty
            && self.retry_not_before.is_none_or(|at| now >= at)
            && self.writing.is_empty()
            && self
                .last_activity
                .is_some_and(|last| now.saturating_duration_since(last) >= debounce)
    }

    fn mark_writing(&mut self, path: PathBuf, now: Instant) {
        if self.writing.len() >= MAX_DIRTY_PATHS && !self.writing.contains_key(&path) {
            self.overflowed = true;
            self.dirty = true;
            self.touch(now);
            return;
        }
        self.writing
            .entry(path)
            .and_modify(|pending| {
                pending.last_check = now;
                pending.observed = None;
                pending.stable_checks = 0;
            })
            .or_insert(PendingWrite {
                first_event: now,
                last_check: now,
                observed: None,
                stable_checks: 0,
            });
        self.touch(now);
    }

    fn verify_unclosed_writes(
        &mut self,
        now: Instant,
        check_interval: Duration,
        timeout: Duration,
    ) -> Result<(), usize> {
        let mut timed_out = 0;
        for (path, pending) in &mut self.writing {
            let stamp = match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    Some(FileStamp::from_metadata(&metadata))
                }
                // The name may disappear/change after draining inotify but
                // before this check. Keep its write until CLOSE_WRITE, DELETE
                // or the paired move events resolve it; metadata is not proof
                // that a writer closed. Missing events still fail closed below.
                Ok(_) | Err(_) => None,
            };
            if now.saturating_duration_since(pending.last_check) >= check_interval {
                if stamp == pending.observed {
                    pending.stable_checks += 1;
                } else {
                    pending.observed = stamp;
                    pending.stable_checks = 0;
                }
                pending.last_check = now;
            }
            if now.saturating_duration_since(pending.first_event) >= timeout {
                timed_out += 1;
            }
        }
        if timed_out > 0 {
            Err(timed_out)
        } else {
            Ok(())
        }
    }

    fn clear(&mut self) {
        self.dirty = false;
        self.overflowed = false;
        self.last_activity = None;
        self.paths.clear();
        self.moved_writes.clear();
        self.retry_not_before = None;
    }

    fn remove_path(&mut self, path: &Path) {
        self.writing
            .retain(|candidate, _| candidate != path && !candidate.starts_with(path));
    }
}

/// A reader may observe a draft, but cannot invoke any callback before its
/// owner hands it off following a fully confirmed Save. Drop aborts it.
pub(crate) struct PreparedWatch {
    reader: InotifyReader,
    sources: Vec<SourceIdentity>,
}

impl PreparedWatch {
    fn prepare(
        roots: Vec<PathBuf>,
        shutdown: &AtomicBool,
        control: &WatcherControl,
    ) -> Result<Self, String> {
        let sources = roots
            .into_iter()
            .map(SourceIdentity::capture)
            .collect::<Result<Vec<_>, _>>()?;
        let reader =
            InotifyReader::new(&sources, control, shutdown)?.ok_or("LOCAL_WATCH_SHUTDOWN")?;
        if !sources.iter().all(SourceIdentity::is_available) {
            return Err("LOCAL_WATCH_SOURCE_CHANGED".into());
        }
        Ok(Self { reader, sources })
    }
    fn bound_to(&self, context: &Context) -> bool {
        self.sources.len() == context.roots().len()
            && self.sources.iter().all(|s| {
                context.sources.values().any(|c| {
                    c.root == s.physical_path && c.device == s.device && c.inode == s.inode
                })
            })
    }
    fn matches(&self, context: &Context) -> bool {
        self.bound_to(context) && self.sources.iter().all(SourceIdentity::is_available)
    }
}

struct Install {
    context: Context,
    prepared: Option<PreparedWatch>,
}

type Mailbox = Arc<Mutex<Option<Install>>>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reconcile {
    Confirmed,
    Busy,
    // External changes await the user's incremental check; this is not a
    // failed monitor and never authorizes an automatic catalog update.
    ChangesPending,
    Blocked,
}

// Information-only notices must neither set an alarm nor clear a real issue.
pub(crate) fn notice_needs_check(reason: &str) -> Option<bool> {
    match reason {
        "ready" => Some(false),
        "changes_pending" => None,
        _ => Some(true),
    }
}

pub(crate) fn reconcile_manual_changes(
    own_writes: &crate::local_own_writes::OwnWrites,
    batch: Batch,
) -> Reconcile {
    match own_writes.filter(batch) {
        None => Reconcile::Busy,
        Some(batch) if batch.paths.is_empty() => Reconcile::Confirmed,
        Some(_) => Reconcile::ChangesPending,
    }
}

pub(crate) struct LocalWatcher {
    control: Arc<WatcherControl>,
    _registration: Arc<(Mutex<Option<bool>>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

fn publish_registration(registration: &Arc<(Mutex<Option<bool>>, Condvar)>, ready: bool) {
    let (state, changed) = &**registration;
    if let Ok(mut state) = state.lock() {
        *state = Some(ready);
        changed.notify_all();
    }
}

impl LocalWatcher {
    fn start_context<F, N>(
        context: Context,
        mailbox: Mailbox,
        debounce: Duration,
        poll_interval: Duration,
        unclosed_write_check_interval: Duration,
        unclosed_write_timeout: Duration,
        shutdown: Arc<AtomicBool>,
        on_change: F,
        needs_check: N,
    ) -> Result<Self, String>
    where
        F: Fn(Batch) -> Reconcile + Send + 'static,
        N: Fn(&'static str) + Send + 'static,
    {
        let control = Arc::new(WatcherControl::new());
        let worker_control = control.clone();
        let registration = Arc::new((Mutex::new(None), Condvar::new()));
        let worker_registration = registration.clone();
        let worker = thread::Builder::new()
            .name("sonante-local-watcher".into())
            .spawn(move || {
                let roots = context.roots();
                let sources = roots
                    .into_iter()
                    .map(SourceIdentity::capture)
                    .collect::<Result<Vec<_>, _>>();
                let sources = match sources {
                    Ok(s) => s,
                    Err(_) => {
                        publish_registration(&worker_registration, false);
                        needs_check("source_unavailable");
                        return;
                    }
                };
                let mut reader = match InotifyReader::new(&sources, &worker_control, &shutdown) {
                    Ok(Some(r)) => r,
                    Ok(None) => {
                        publish_registration(&worker_registration, false);
                        return;
                    }
                    Err(_) => {
                        publish_registration(&worker_registration, false);
                        needs_check("registration_failed");
                        return;
                    }
                };
                publish_registration(&worker_registration, true);
                needs_check("ready");
                run_watcher(
                    &mut reader,
                    context,
                    mailbox,
                    &worker_control,
                    &shutdown,
                    debounce,
                    poll_interval,
                    unclosed_write_check_interval,
                    unclosed_write_timeout,
                    on_change,
                    needs_check,
                );
            })
            .map_err(|_| "LOCAL_WATCH_WORKER_UNAVAILABLE")?;
        Ok(Self {
            control,
            _registration: registration,
            worker: Some(worker),
        })
    }

    #[cfg(test)]
    fn start_with_timing<F>(
        roots: Vec<PathBuf>,
        debounce: Duration,
        poll_interval: Duration,
        unclosed_write_check_interval: Duration,
        unclosed_write_timeout: Duration,
        shutdown: Arc<AtomicBool>,
        on_change: F,
    ) -> Result<Self, String>
    where
        F: Fn() -> Result<(), String> + Send + 'static,
    {
        Self::start_context(
            Context::for_roots(&roots),
            Arc::new(Mutex::new(None)),
            debounce,
            poll_interval,
            unclosed_write_check_interval,
            unclosed_write_timeout,
            shutdown,
            move |_| {
                if on_change().is_ok() {
                    Reconcile::Confirmed
                } else {
                    Reconcile::Blocked
                }
            },
            |_| {},
        )
    }

    pub(crate) fn request_stop(&self) {
        self.control.stop();
    }
    #[cfg(test)]
    fn wait_until_ready(&self, timeout: Duration) -> bool {
        let (state, changed) = &*self._registration;
        let Ok(state) = state.lock() else {
            return false;
        };
        let Ok((state, _)) = changed.wait_timeout_while(state, timeout, |ready| ready.is_none())
        else {
            return false;
        };
        *state == Some(true)
    }
    pub(crate) fn join(mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("[Watcher] worker panicked");
            }
        }
    }
}

pub(crate) struct LocalWatcherState {
    watcher: Mutex<Option<LocalWatcher>>,
    mailbox: Mailbox,
    needs_check: Arc<AtomicBool>,
    preparation_control: Arc<WatcherControl>,
    pub(crate) own_writes: Arc<crate::local_own_writes::OwnWrites>,
    shutdown: Arc<AtomicBool>,
}

impl LocalWatcherState {
    pub(crate) fn new() -> Self {
        Self {
            watcher: Mutex::new(None),
            mailbox: Arc::new(Mutex::new(None)),
            own_writes: Arc::new(crate::local_own_writes::OwnWrites::default()),
            needs_check: Arc::new(AtomicBool::new(false)),
            preparation_control: Arc::new(WatcherControl::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn prepare(&self, roots: Vec<PathBuf>) -> Result<PreparedWatch, String> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err("LOCAL_WATCH_SHUTDOWN".into());
        }
        if self
            .mailbox
            .lock()
            .map_err(|_| "LOCAL_WATCH_MAILBOX_UNAVAILABLE")?
            .as_ref()
            .is_some_and(|i| i.prepared.is_some())
        {
            return Err("LOCAL_WATCH_HANDOFF_PENDING".into());
        }
        let roots = crate::local_media::mirror::canonical_configured_roots(&roots)?;
        PreparedWatch::prepare(roots, &self.shutdown, &self.preparation_control)
    }

    /// No join or callback under a state/transaction mutex. At most one pending
    /// prepared reader. A later context-only publication can rebase it; a
    /// second Save is refused before effects until handoff has been consumed.
    pub(crate) fn install(
        &self,
        context: Context,
        prepared: Option<PreparedWatch>,
    ) -> Result<(), String> {
        // Physical I/O precedes the mailbox lock; the worker revalidates
        // again after taking ownership, outside every state mutex.
        if prepared.as_ref().is_some_and(|p| !p.matches(&context)) {
            return Err("LOCAL_WATCH_SOURCE_CHANGED".into());
        }
        let mut mailbox = self
            .mailbox
            .lock()
            .map_err(|_| "LOCAL_WATCH_MAILBOX_UNAVAILABLE")?;
        if self.shutdown.load(Ordering::SeqCst) {
            return Err("LOCAL_WATCH_SHUTDOWN".into());
        }
        if prepared.is_some() && mailbox.as_ref().is_some_and(|i| i.prepared.is_some()) {
            return Err("LOCAL_WATCH_HANDOFF_PENDING".into());
        }
        if prepared.is_none()
            && mailbox
                .as_ref()
                .and_then(|i| i.prepared.as_ref())
                .is_some_and(|p| !p.bound_to(&context))
        {
            return Err("LOCAL_WATCH_SOURCE_CHANGED".into());
        }
        // A namespace-only/startup reconciliation must not overwrite a queued
        // prepared reader. Preserve it when it covers the same physical roots.
        let prepared = match prepared {
            Some(p) => Some(p),
            None => mailbox.as_mut().and_then(|i| i.prepared.take()),
        };
        *mailbox = Some(Install { context, prepared });
        Ok(())
    }

    pub(crate) fn start_context<F, N>(
        &self,
        context: Context,
        on_change: F,
        needs_check: N,
    ) -> Result<(), String>
    where
        F: Fn(Batch) -> Reconcile + Send + 'static,
        N: Fn(&'static str) + Send + 'static,
    {
        let finished = {
            let mut current = self
                .watcher
                .lock()
                .map_err(|_| "LOCAL_WATCH_STATE_UNAVAILABLE")?;
            if self.shutdown.load(Ordering::SeqCst) {
                return Err("LOCAL_WATCH_SHUTDOWN".into());
            }
            if current
                .as_ref()
                .is_some_and(|w| w.worker.as_ref().is_some_and(|w| !w.is_finished()))
            {
                return Ok(());
            }
            current.take()
        };
        if let Some(finished) = finished {
            finished.join();
        }
        let mut current = self
            .watcher
            .lock()
            .map_err(|_| "LOCAL_WATCH_STATE_UNAVAILABLE")?;
        if self.shutdown.load(Ordering::SeqCst) {
            return Err("LOCAL_WATCH_SHUTDOWN".into());
        }
        if current.is_some() {
            return Ok(());
        }
        let status = self.needs_check.clone();
        *current = Some(LocalWatcher::start_context(
            context,
            self.mailbox.clone(),
            WATCH_DEBOUNCE,
            WATCH_POLL_INTERVAL,
            UNCLOSED_WRITE_CHECK_INTERVAL,
            UNCLOSED_WRITE_TIMEOUT,
            self.shutdown.clone(),
            on_change,
            move |reason| {
                if let Some(needs_check) = notice_needs_check(reason) {
                    status.store(needs_check, Ordering::SeqCst);
                }
                needs_check(reason);
            },
        )?);
        Ok(())
    }

    #[cfg(test)]
    fn start<F>(&self, roots: Vec<PathBuf>, on_change: F) -> Result<(), String>
    where
        F: Fn() -> Result<(), String> + Send + 'static,
    {
        self.start_context(
            Context::for_roots(&roots),
            move |_| {
                if on_change().is_ok() {
                    Reconcile::Confirmed
                } else {
                    Reconcile::Blocked
                }
            },
            |_| {},
        )
    }

    pub(crate) fn running(&self) -> bool {
        self.watcher.lock().ok().is_some_and(|w| {
            w.as_ref()
                .is_some_and(|w| w.worker.as_ref().is_some_and(|w| !w.is_finished()))
        })
    }
    pub(crate) fn needs_check(&self) -> bool {
        self.needs_check.load(Ordering::SeqCst)
    }
    pub(crate) fn mark_needs_check(&self) {
        self.needs_check.store(true, Ordering::SeqCst);
    }

    pub(crate) fn begin_shutdown(&self) {
        // Fence install under the same mailbox lock; recursive registration
        // checks the atomic at every directory and never activates after this.
        let abandoned = if let Ok(mut mailbox) = self.mailbox.lock() {
            self.shutdown.store(true, Ordering::SeqCst);
            mailbox.take()
        } else {
            self.shutdown.store(true, Ordering::SeqCst);
            None
        };
        drop(abandoned); // Closing registrations is never done under mailbox.
        self.preparation_control.stop();
        if let Ok(current) = self.watcher.lock() {
            if let Some(w) = current.as_ref() {
                w.request_stop();
            }
        }
    }
    pub(crate) fn stop_and_join(&self) {
        let watcher = self.watcher.lock().ok().and_then(|mut c| c.take());
        if let Some(w) = watcher {
            w.join();
        }
    }
}

fn run_watcher<F, N>(
    reader: &mut InotifyReader,
    mut context: Context,
    mailbox: Mailbox,
    control: &WatcherControl,
    shutdown: &AtomicBool,
    debounce: Duration,
    poll_interval: Duration,
    unclosed_write_check_interval: Duration,
    unclosed_write_timeout: Duration,
    on_change: F,
    needs_check: N,
) where
    F: Fn(Batch) -> Reconcile,
    N: Fn(&'static str),
{
    let mut pending = PendingChanges::default();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut paused = false;
    while !control.stopped(shutdown) {
        let install = match mailbox.lock() {
            Ok(mut m) => m.take(),
            Err(_) => {
                needs_check("mailbox_unavailable");
                return;
            }
        };
        if let Some(install) = install {
            if control.stopped(shutdown) {
                return;
            }
            if let Some(prepared) = install.prepared {
                if !prepared.matches(&install.context) {
                    needs_check("source_changed");
                    return;
                }
                // New reader was registered before Save's scan. Its queued
                // events cover the full handoff, including surviving sources.
                *reader = prepared.reader;
                if paused {
                    pending = PendingChanges::default();
                }
                paused = false;
                needs_check("ready");
            } else if context.sources != install.context.sources {
                if !install.context.is_source_removal_from(&context) || !install.context.available()
                    || reader.retain_roots(&install.context.roots()).is_err() {
                    needs_check("unprepared_roots_change");
                    return;
                }
            }
            pending.rebase(&context, &install.context);
            context = install.context;
        }
        if paused {
            thread::park_timeout(poll_interval);
            continue;
        }
        let mut descriptor = libc::pollfd {
            fd: reader.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = poll_interval.as_millis().min(i32::MAX as u128) as i32;
        let polled = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if polled < 0 {
            if std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
                needs_check("inotify_unavailable");
                return;
            }
        } else if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            needs_check("inotify_unavailable");
            return;
        } else if polled > 0 && descriptor.revents & libc::POLLIN != 0 {
            loop {
                if control.stopped(shutdown) {
                    return;
                }
                match (&reader.fd).read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        process_events(reader, control, shutdown, &buffer[..length], &mut pending)
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => {
                        needs_check("inotify_unavailable");
                        return;
                    }
                }
            }
        }
        let now = Instant::now();
        pending
            .moved_writes
            .retain(|_, (when, _)| now.saturating_duration_since(*when) < debounce);
        if pending
            .verify_unclosed_writes(now, unclosed_write_check_interval, unclosed_write_timeout)
            .is_err()
        {
            needs_check("write_not_closed");
            paused = true;
            continue;
        }
        if pending.overflowed {
            needs_check("overflow");
            paused = true;
            continue;
        }
        if pending.ready(now, debounce) {
            if !context.available() {
                needs_check("source_unavailable");
                paused = true;
                continue;
            }
            if control.stopped(shutdown) {
                return;
            }
            match on_change(Batch {
                context: context.clone(),
                paths: pending.paths.clone(),
            }) {
                Reconcile::Confirmed => pending.clear(),
                Reconcile::Busy => {
                    // No attempt ceiling: bounded memory and at most 1 retry/s.
                    // poll continues draining/coalescing events and shutdown.
                    pending.retry_not_before = Some(Instant::now() + BUSY_RETRY_INTERVAL);
                }
                Reconcile::ChangesPending => {
                    needs_check("changes_pending");
                    paused = true;
                }
                Reconcile::Blocked => {
                    needs_check("reconciliation_blocked");
                    paused = true;
                }
            }
        }
    }
}

fn process_events(
    reader: &mut InotifyReader,
    control: &WatcherControl,
    shutdown: &AtomicBool,
    bytes: &[u8],
    pending: &mut PendingChanges,
) {
    let event_size = std::mem::size_of::<libc::inotify_event>();
    let mut offset = 0;
    while offset + event_size <= bytes.len() {
        let event = unsafe {
            std::ptr::read_unaligned(bytes[offset..].as_ptr().cast::<libc::inotify_event>())
        };
        let record_length = event_size.saturating_add(event.len as usize);
        if record_length < event_size || offset + record_length > bytes.len() {
            pending.overflowed = true;
            return;
        }
        let now = Instant::now();
        if event.mask & libc::IN_Q_OVERFLOW != 0 {
            pending.dirty = true;
            pending.overflowed = true;
            pending.writing.clear();
            pending.touch(now);
            offset += record_length;
            continue;
        }
        let Some(directory) = reader.directories.get(&event.wd).cloned() else {
            offset += record_length;
            continue;
        };
        let name_bytes = &bytes[offset + event_size..offset + record_length];
        let name_length = name_bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name_bytes.len());
        let path = if name_length == 0 {
            directory.clone()
        } else {
            directory.join(std::ffi::OsStr::from_bytes(&name_bytes[..name_length]))
        };
        let is_directory = event.mask & libc::IN_ISDIR != 0;

        if event.mask & libc::IN_IGNORED != 0 {
            reader.directories.remove(&event.wd);
        }
        if is_directory && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
            match reader.watch_tree(&path, control, shutdown) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    eprintln!(
                        "[Watcher] Failed to extend recursive monitoring: {}",
                        crate::diagnostics::error_summary(&error)
                    );
                    pending.overflowed = true;
                }
            }
            pending.mark_path(path.clone(), now);
        } else if !is_directory && event.mask & (libc::IN_CREATE | libc::IN_MODIFY) != 0 {
            let link_like_create = event.mask & libc::IN_CREATE != 0
                && fs::symlink_metadata(&path)
                    .is_ok_and(|metadata| !metadata.file_type().is_file() || metadata.nlink() > 1);
            if link_like_create {
                pending.mark_path(path.clone(), now);
            } else {
                pending.mark_writing(path.clone(), now);
            }
        }
        if !is_directory && event.mask & libc::IN_CLOSE_WRITE != 0 {
            pending.writing.remove(&path);
            pending.mark_path(path.clone(), now);
        }
        if event.mask & (libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_DELETE) != 0 {
            let mut moved_write = None;
            if event.mask & libc::IN_MOVED_FROM != 0 && event.cookie != 0 {
                if pending.moved_writes.len() >= MAX_DIRTY_PATHS {
                    pending.overflowed = true;
                } else {
                    pending
                        .moved_writes
                        .insert(event.cookie, (now, pending.writing.remove(&path)));
                }
            }
            if event.mask & libc::IN_MOVED_TO != 0 && event.cookie != 0 {
                moved_write = pending
                    .moved_writes
                    .remove(&event.cookie)
                    .and_then(|(_, write)| write);
            }
            pending.remove_path(&path);
            if let Some(write) = moved_write {
                pending.writing.insert(path.clone(), write);
            }
            pending.mark_path(path.clone(), now);
        }
        if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_UNMOUNT) != 0 {
            pending.remove_path(&directory);
            pending.mark_path(directory.clone(), now);
        }
        offset += record_length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sonante-watcher-{}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn wait_for_count(count: &AtomicUsize, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while count.load(Ordering::SeqCst) < expected && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(count.load(Ordering::SeqCst), expected);
    }

    fn test_watcher(root: &Path, count: Arc<AtomicUsize>) -> LocalWatcher {
        let watcher = LocalWatcher::start_with_timing(
            vec![root.to_path_buf()],
            Duration::from_millis(80),
            Duration::from_millis(10),
            Duration::from_millis(100),
            Duration::from_secs(2),
            Arc::new(AtomicBool::new(false)),
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
        assert!(watcher.wait_until_ready(Duration::from_secs(3)));
        watcher
    }

    #[test]
    fn addition_and_removal_are_debounced_into_confirmed_changes() {
        let root = fixture();
        let nested = root.join("Artist").join("Album");
        fs::create_dir_all(&nested).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());

        fs::write(nested.join("track.flac"), b"audio").unwrap();
        wait_for_count(&count, 1);
        fs::remove_file(nested.join("track.flac")).unwrap();
        wait_for_count(&count, 2);

        watcher.join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_events_during_copy_produce_one_reconciliation() {
        let root = fixture();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());

        let path = root.join("track.flac");
        let mut file = File::create(&path).unwrap();
        for payload in [b"a".as_slice(), b"ab", b"abc", b"finished"] {
            file.write_all(payload).unwrap();
            file.flush().unwrap();
            thread::sleep(Duration::from_millis(15));
            assert_eq!(count.load(Ordering::SeqCst), 0);
        }
        drop(file);
        wait_for_count(&count, 1);
        thread::sleep(Duration::from_millis(120));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        watcher.join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_events_share_a_debounce_window_but_later_batches_are_distinct() {
        let started = Instant::now();
        let debounce = Duration::from_millis(80);
        let mut pending = PendingChanges::default();

        // Completed events (e.g. CLOSE_WRITE) mark dirty and touch the timer.
        pending.dirty = true;
        pending.touch(started);
        assert!(!pending.ready(started + Duration::from_millis(79), debounce));
        pending.touch(started + Duration::from_millis(60));
        assert!(!pending.ready(started + debounce, debounce));
        assert!(!pending.ready(started + Duration::from_millis(139), debounce));
        assert!(pending.ready(started + Duration::from_millis(140), debounce));

        // run_watcher clears a reconciled batch; elapsed time alone cannot repeat it.
        pending.clear();
        assert!(!pending.ready(started + Duration::from_millis(221), debounce));
        pending.dirty = true;
        pending.touch(started + Duration::from_millis(221));
        assert!(!pending.ready(started + Duration::from_millis(300), debounce));
        assert!(pending.ready(started + Duration::from_millis(301), debounce));
        pending.clear();
        assert!(!pending.ready(started + Duration::from_millis(381), debounce));
    }

    #[test]
    fn open_file_remains_blocked_past_twice_the_check_interval_until_close_write() {
        let root = fixture();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());

        let mut file = File::create(root.join("writing.flac")).unwrap();
        file.write_all(b"partial audio").unwrap();
        file.flush().unwrap();
        thread::sleep(Duration::from_millis(600));
        assert_eq!(count.load(Ordering::SeqCst), 0);
        drop(file);
        wait_for_count(&count, 1);

        watcher.join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn link_creation_without_close_write_is_ready_and_later_events_continue() {
        let root = fixture();
        let original = root.join("original.flac");
        fs::write(&original, b"audio").unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());

        std::os::unix::fs::symlink(&original, root.join("linked.flac")).unwrap();
        fs::hard_link(&original, root.join("hard-linked.flac")).unwrap();
        wait_for_count(&count, 1);

        fs::write(root.join("later.flac"), b"later audio").unwrap();
        wait_for_count(&count, 2);

        watcher.join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_rename_into_source_is_treated_as_complete_publication() {
        let root = fixture();
        let staged = root.with_extension("staged.flac");
        fs::write(&staged, b"complete audio").unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());

        fs::rename(&staged, root.join("published.flac")).unwrap();
        wait_for_count(&count, 1);

        watcher.join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unresolved_regular_event_is_reported_without_becoming_ready() {
        let root = fixture();
        let path = root.join("without-close.flac");
        fs::write(&path, b"complete audio").unwrap();
        let started = Instant::now();
        let check_interval = Duration::from_millis(50);
        let timeout = Duration::from_millis(500);
        let mut pending = PendingChanges::default();
        pending.mark_writing(path, started);

        for multiplier in 1..=3 {
            pending
                .verify_unclosed_writes(
                    started + check_interval * multiplier,
                    check_interval,
                    timeout,
                )
                .unwrap();
        }

        assert_eq!(pending.writing.len(), 1);
        assert!(pending
            .writing
            .values()
            .all(|pending| pending.stable_checks >= 2));
        assert!(!pending.dirty);
        assert_eq!(
            pending.verify_unclosed_writes(started + timeout, check_interval, timeout),
            Err(1)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_stops_future_reconciliations() {
        let root = fixture();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());
        watcher.join();

        fs::write(root.join("after-stop.flac"), b"audio").unwrap();
        thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_during_recursive_registration_never_activates_late() {
        let root = fixture();
        for index in 0..500 {
            fs::create_dir(root.join(format!("directory-{index}"))).unwrap();
        }
        let count = Arc::new(AtomicUsize::new(0));
        let callback_count = count.clone();
        let state = LocalWatcherState::new();
        state
            .start(vec![root.clone()], move || {
                callback_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();

        state.begin_shutdown();
        state.stop_and_join();
        fs::write(root.join("after-shutdown.flac"), b"audio").unwrap();
        thread::sleep(Duration::from_millis(150));

        assert_eq!(count.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_source_never_becomes_a_mass_deletion() {
        let root = fixture();
        fs::write(root.join("existing.flac"), b"audio").unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let watcher = test_watcher(&root, count.clone());
        let unavailable = root.with_extension("unavailable");

        fs::rename(&root, &unavailable).unwrap();
        thread::sleep(Duration::from_millis(250));
        assert_eq!(count.load(Ordering::SeqCst), 0);

        watcher.join();
        fs::remove_dir_all(unavailable).unwrap();
    }

    #[test]
    fn indexed_source_removal_worker_handoff_keeps_survivor_events_without_new_registration() {
        let root=fixture();let keep=root.join("Keep");let removed=root.join("Remove");
        fs::create_dir_all(&keep).unwrap();fs::create_dir_all(&removed).unwrap();
        let before=Context::for_roots(&[keep.clone(),removed.clone()]);let mut after=before.clone();after.sources.retain(|_,s|s.root==keep);
        let mailbox=Arc::new(Mutex::new(None));let (send,receive)=std::sync::mpsc::channel();
        let watcher=LocalWatcher::start_context(before,mailbox.clone(),Duration::from_millis(80),Duration::from_millis(10),
            Duration::from_millis(100),Duration::from_secs(2),Arc::new(AtomicBool::new(false)),
            move |batch| {let _=send.send((batch.context.sources.len(),batch.paths));Reconcile::Confirmed}, |_|{}).unwrap();
        assert!(watcher.wait_until_ready(Duration::from_secs(3)));
        *mailbox.lock().unwrap()=Some(Install{context:after,prepared:None});
        fs::write(removed.join("changed.flac"),b"removed event").unwrap();fs::write(keep.join("changed.flac"),b"survivor event").unwrap();
        let observed=receive.recv_timeout(Duration::from_secs(3));
        fs::write(removed.join("later.flac"),b"retired event").unwrap();
        let late=receive.recv_timeout(Duration::from_millis(200));
        watcher.join();fs::remove_dir_all(root).unwrap();
        let (sources,paths)=observed.unwrap();assert_eq!(sources,1);
        assert!(paths.contains(&keep.join("changed.flac")));assert!(!paths.iter().any(|p|p.starts_with(&removed)));
        assert!(late.is_err(),"retired roots must not generate a survivor notification");
    }

    #[test]
    fn indexed_source_removal_retires_only_owned_watches_and_keeps_pending_survivors() {
        let root=fixture();let keep=root.join("Keep");let removed=root.join("Remove");
        fs::create_dir_all(keep.join("Album")).unwrap();fs::create_dir_all(removed.join("Album")).unwrap();
        let sources=vec![SourceIdentity::capture(keep.clone()).unwrap(),SourceIdentity::capture(removed.clone()).unwrap()];
        let control=WatcherControl::new();let shutdown=AtomicBool::new(false);
        let mut reader=InotifyReader::new(&sources,&control,&shutdown).unwrap().unwrap();
        let before=reader.directories.iter().filter(|(_,p)|p.starts_with(&keep)).map(|(wd,p)|(*wd,p.clone())).collect::<HashMap<_,_>>();
        let previous=Context::for_roots(&[keep.clone(),removed.clone()]);let mut current=previous.clone();current.sources.retain(|_,s|s.root==keep);
        assert!(current.is_source_removal_from(&previous));assert!(!previous.is_source_removal_from(&current));
        let mut changed=current.clone();changed.sources.values_mut().next().unwrap().inode+=1;
        assert!(!changed.is_source_removal_from(&previous));
        let mut pending=PendingChanges::default();pending.mark_path(keep.join("Album/a.flac"),Instant::now());pending.mark_path(removed.join("Album/b.flac"),Instant::now());
        reader.retain_roots(&current.roots()).unwrap();pending.rebase(&previous,&current);
        assert_eq!(reader.directories,before);assert_eq!(pending.paths,BTreeSet::from([keep.join("Album/a.flac")]));
        reader.retain_roots(&[]).unwrap();assert!(reader.directories.is_empty());
        drop(reader);fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn overflow_is_marked_for_explicit_manual_recovery() {
        let root = fixture();
        let sources = vec![SourceIdentity::capture(root.clone()).unwrap()];
        let control = WatcherControl::new();
        let shutdown = AtomicBool::new(false);
        let mut reader = InotifyReader::new(&sources, &control, &shutdown)
            .unwrap()
            .unwrap();
        let event = libc::inotify_event {
            wd: -1,
            mask: libc::IN_Q_OVERFLOW,
            cookie: 0,
            len: 0,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&event as *const libc::inotify_event).cast::<u8>(),
                std::mem::size_of::<libc::inotify_event>(),
            )
        };
        let mut pending = PendingChanges::default();
        pending.mark_writing(root.join("possibly-writing.flac"), Instant::now());

        process_events(&mut reader, &control, &shutdown, bytes, &mut pending);

        assert!(pending.dirty);
        assert!(pending.overflowed);
        assert!(pending.writing.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "local_watcher_updates_tests.rs"]
mod updates_tests;
