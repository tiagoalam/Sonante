use std::collections::{HashMap, HashSet};
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
    last_activity: Option<Instant>,
}

impl PendingChanges {
    fn touch(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    fn ready(&self, now: Instant, debounce: Duration) -> bool {
        self.dirty
            && self.writing.is_empty()
            && self
                .last_activity
                .is_some_and(|last| now.saturating_duration_since(last) >= debounce)
    }

    fn mark_writing(&mut self, path: PathBuf, now: Instant) {
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
        let mut removed = Vec::new();
        let mut timed_out = 0;
        for (path, pending) in &mut self.writing {
            let stamp = match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    Some(FileStamp::from_metadata(&metadata))
                }
                Ok(_) => {
                    removed.push(path.clone());
                    continue;
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    removed.push(path.clone());
                    continue;
                }
                Err(_) => None,
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
        if !removed.is_empty() {
            for path in removed {
                self.writing.remove(&path);
            }
            self.dirty = true;
            self.touch(now);
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
    }

    fn remove_path(&mut self, path: &Path) {
        self.writing
            .retain(|candidate, _| candidate != path && !candidate.starts_with(path));
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
        let control = Arc::new(WatcherControl::new());
        let worker_control = control.clone();
        let registration = Arc::new((Mutex::new(None), Condvar::new()));
        let worker_registration = registration.clone();
        let worker = thread::Builder::new()
            .name("sonante-local-watcher".to_string())
            .spawn(move || {
                let mut sources = Vec::with_capacity(roots.len());
                for root in roots {
                    if worker_control.stopped(&shutdown) {
                        publish_registration(&worker_registration, false);
                        return;
                    }
                    match SourceIdentity::capture(root) {
                        Ok(source) => sources.push(source),
                        Err(error) => {
                            if !worker_control.stopped(&shutdown) {
                                eprintln!(
                                    "[Watcher] Failed to prepare local sources: {}",
                                    crate::diagnostics::error_summary(&error)
                                );
                            }
                            publish_registration(&worker_registration, false);
                            return;
                        }
                    }
                }
                let mut reader = match InotifyReader::new(&sources, &worker_control, &shutdown) {
                    Ok(Some(reader)) => reader,
                    Ok(None) => {
                        publish_registration(&worker_registration, false);
                        return;
                    }
                    Err(error) => {
                        if !worker_control.stopped(&shutdown) {
                            eprintln!(
                                "[Watcher] Failed to register local sources: {}",
                                crate::diagnostics::error_summary(&error)
                            );
                        }
                        publish_registration(&worker_registration, false);
                        return;
                    }
                };
                publish_registration(&worker_registration, true);
                run_watcher(
                    &mut reader,
                    &sources,
                    &worker_control,
                    &shutdown,
                    debounce,
                    poll_interval,
                    unclosed_write_check_interval,
                    unclosed_write_timeout,
                    on_change,
                )
            })
            .map_err(|error| format!("Falha ao iniciar worker do inotify: {error}"))?;
        Ok(Self {
            control,
            _registration: registration,
            worker: Some(worker),
        })
    }

    pub(crate) fn start<F>(
        roots: Vec<PathBuf>,
        shutdown: Arc<AtomicBool>,
        on_change: F,
    ) -> Result<Self, String>
    where
        F: Fn() -> Result<(), String> + Send + 'static,
    {
        Self::start_with_timing(
            roots,
            WATCH_DEBOUNCE,
            WATCH_POLL_INTERVAL,
            UNCLOSED_WRITE_CHECK_INTERVAL,
            UNCLOSED_WRITE_TIMEOUT,
            shutdown,
            on_change,
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
                eprintln!("[Watcher] Inotify worker panicked.");
            }
        }
    }
}

pub(crate) struct LocalWatcherState {
    watcher: Mutex<Option<LocalWatcher>>,
    shutdown: Arc<AtomicBool>,
}

impl LocalWatcherState {
    pub(crate) fn new() -> Self {
        Self {
            watcher: Mutex::new(None),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn start<F>(&self, roots: Vec<PathBuf>, on_change: F) -> Result<(), String>
    where
        F: Fn() -> Result<(), String> + Send + 'static,
    {
        if roots.is_empty() {
            return Ok(());
        }
        let mut current = self
            .watcher
            .lock()
            .map_err(|error| format!("Falha ao acessar estado do watcher local: {error}"))?;
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(
                "A aplicação está sendo encerrada; o watcher local não foi iniciado.".to_string(),
            );
        }
        if current.is_some() {
            return Err("O watcher local já foi iniciado.".to_string());
        }
        let watcher = LocalWatcher::start(roots, self.shutdown.clone(), on_change)?;
        *current = Some(watcher);
        Ok(())
    }

    pub(crate) fn begin_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Ok(current) = self.watcher.lock() {
            if let Some(watcher) = current.as_ref() {
                watcher.request_stop();
            }
        }
    }

    pub(crate) fn stop_and_join(&self) {
        let watcher = self
            .watcher
            .lock()
            .ok()
            .and_then(|mut current| current.take());
        if let Some(watcher) = watcher {
            watcher.join();
        }
    }
}

fn run_watcher<F>(
    reader: &mut InotifyReader,
    sources: &[SourceIdentity],
    control: &WatcherControl,
    shutdown: &AtomicBool,
    debounce: Duration,
    poll_interval: Duration,
    unclosed_write_check_interval: Duration,
    unclosed_write_timeout: Duration,
    on_change: F,
) where
    F: Fn() -> Result<(), String>,
{
    let mut pending = PendingChanges::default();
    let mut buffer = vec![0_u8; 64 * 1024];
    while !control.stopped(shutdown) {
        let mut descriptor = libc::pollfd {
            fd: reader.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = poll_interval.as_millis().min(i32::MAX as u128) as i32;
        let poll_result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if poll_result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                eprintln!(
                    "[Watcher] Failed to wait for inotify events: {}",
                    crate::diagnostics::error_summary(&error)
                );
                break;
            }
        } else if poll_result > 0 && descriptor.revents & libc::POLLIN != 0 {
            loop {
                match (&reader.fd).read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        process_events(reader, control, shutdown, &buffer[..length], &mut pending)
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => {
                        eprintln!(
                            "[Watcher] Failed to read inotify events: {}",
                            crate::diagnostics::error_summary(&error)
                        );
                        return;
                    }
                }
            }
        } else if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            eprintln!("[Watcher] Inotify descriptor became unavailable.");
            break;
        }

        let now = Instant::now();
        if let Err(count) = pending.verify_unclosed_writes(
            now,
            unclosed_write_check_interval,
            unclosed_write_timeout,
        ) {
            eprintln!(
                "[Watcher] {count} regular file(s) did not emit CLOSE_WRITE within the limit; the watcher stopped without reconciliation to avoid partial content. Run a manual rescan after confirming writes have finished."
            );
            return;
        }
        if pending.ready(now, debounce) {
            if !sources.iter().all(SourceIdentity::is_available) {
                eprintln!(
                    "[Watcher] Reconciliation skipped: one or more local sources became unavailable or changed identity. Run a manual rescan after restoring them."
                );
                pending.clear();
                continue;
            }
            if pending.overflowed {
                eprintln!(
                    "[Watcher] Inotify overflow detected; automatic reconciliation was skipped because files may still be written. Run a manual rescan."
                );
                pending.clear();
                continue;
            }
            if let Err(error) = on_change() {
                eprintln!(
                    "[Watcher] Automatic reconciliation failed: {}",
                    crate::diagnostics::error_summary(&error)
                );
            }
            pending.clear();
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
            eprintln!("[Watcher] Truncated or invalid inotify event.");
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
            pending.dirty = true;
            pending.touch(now);
        } else if !is_directory && event.mask & (libc::IN_CREATE | libc::IN_MODIFY) != 0 {
            let link_like_create = event.mask & libc::IN_CREATE != 0
                && fs::symlink_metadata(&path)
                    .is_ok_and(|metadata| !metadata.file_type().is_file() || metadata.nlink() > 1);
            if link_like_create {
                pending.dirty = true;
                pending.touch(now);
            } else {
                pending.mark_writing(path.clone(), now);
            }
        }
        if !is_directory && event.mask & libc::IN_CLOSE_WRITE != 0 {
            pending.writing.remove(&path);
            pending.dirty = true;
            pending.touch(now);
        }
        if event.mask & (libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_DELETE) != 0 {
            pending.remove_path(&path);
            pending.dirty = true;
            pending.touch(now);
        }
        if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_UNMOUNT) != 0 {
            pending.remove_path(&directory);
            pending.dirty = true;
            pending.touch(now);
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
        for payload in [b"a".as_slice(), b"ab", b"abc", b"finished"] {
            fs::write(&path, payload).unwrap();
            thread::sleep(Duration::from_millis(15));
        }
        wait_for_count(&count, 1);
        thread::sleep(Duration::from_millis(120));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        watcher.join();
        fs::remove_dir_all(root).unwrap();
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
