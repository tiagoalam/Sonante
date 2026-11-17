//! Windows persistence for the original artwork-progress service. No images,
//! media, online cache, MPD or audio. Linux orchestration remains unchanged.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, &'static str>;
const FAILED: &str = "windows_artwork_progress_failed";
const INVALID: &str = "windows_artwork_progress_invalid";
const BUSY: &str = "windows_artwork_progress_busy";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 100000;
const MAX_SECONDS: u64 = 253402300799; // year 9999; bounds persisted clock data.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Local,
    Embedded,
    Online,
    Ineligible,
    None,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub result: ResultKind,
    pub online_complete: bool,
    pub checked_at: u64,
}
#[derive(Deserialize)]
struct Change {
    album_id: String,
    result: ResultKind,
    online_complete: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u8,
    entries: BTreeMap<String, Entry>,
}
fn id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
impl Index {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.entries.len() > MAX_ENTRIES
            || self
                .entries
                .iter()
                .any(|(key, v)| !id(key) || v.checked_at > MAX_SECONDS)
        {
            return Err(INVALID);
        }
        Ok(())
    }
    fn projected(&self) -> BTreeMap<String, Entry> {
        // Same original valid_entries policy with no positive/negative online
        // files: this Windows build does not implement an online cover cache.
        self.entries
            .iter()
            .map(|(id, value)| {
                let mut value = value.clone();
                if matches!(value.result, ResultKind::Online | ResultKind::None) {
                    value.online_complete = false;
                }
                (id.clone(), value)
            })
            .collect()
    }
}
fn read(path: &Path) -> Result<Option<Index>> {
    crate::persistence::prepare_private_file_for_load(path, "artwork progress")
        .map_err(|_| FAILED)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(1)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(FAILED),
    };
    let metadata = file.metadata().map_err(|_| FAILED)?;
    if !metadata.is_file()
        || crate::persistence::linked(&metadata)
        || metadata.len() > MAX_BYTES as u64
    {
        return Err(INVALID);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FAILED)?;
    if bytes.len() > MAX_BYTES {
        return Err(INVALID);
    }
    let index: Index = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
    index.validate()?;
    Ok(Some(index))
}
fn write(path: &Path, index: &Index) -> Result<()> {
    index.validate()?;
    let bytes = serde_json::to_vec(index).map_err(|_| FAILED)?;
    if bytes.len() > MAX_BYTES {
        return Err(INVALID);
    }
    crate::persistence::atomic_write_private(path, &bytes, "artwork progress").map_err(|_| FAILED)
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| FAILED)
}
fn dispatch(path: &Path, operation: &str, args: Value, now: u64) -> Result<Value> {
    let _lock = crate::persistence::operation_lock(&path.with_extension("lock")).map_err(|e| {
        if e == "windows_playlist_busy" {
            BUSY
        } else {
            FAILED
        }
    })?;
    let current = read(path)?;
    match operation {
        "list" => serde_json::to_value(current.map(|v| v.projected())).map_err(|_| FAILED),
        "import" => {
            if let Some(index) = current {
                return serde_json::to_value(index.projected()).map_err(|_| FAILED);
            }
            let legacy = args["legacy"].as_str().ok_or(INVALID)?;
            if legacy.len() > MAX_BYTES {
                return Err(INVALID);
            }
            let mut entries: BTreeMap<String, Entry> =
                serde_json::from_str(legacy).map_err(|_| INVALID)?;
            // Preview8 wrote Date.now() milliseconds, whereas the shared native
            // service contract uses Unix seconds. Convert only legacy input.
            for entry in entries.values_mut() {
                if entry.checked_at >= 1_000_000_000_000 {
                    entry.checked_at /= 1000;
                }
            }
            let index = Index {
                version: 1,
                entries,
            };
            write(path, &index)?;
            serde_json::to_value(index.projected()).map_err(|_| FAILED)
        }
        "save" => {
            let mut index = current.ok_or("windows_artwork_progress_migration_required")?;
            let list = args["changes"].as_array().ok_or(INVALID)?;
            if list.len() > 256 || now > MAX_SECONDS {
                return Err(INVALID);
            }
            let changes: Vec<Change> =
                serde_json::from_value(args["changes"].clone()).map_err(|_| INVALID)?;
            // Validate the whole batch before any mutation or write.
            if changes.iter().any(|v| !id(&v.album_id)) {
                return Err(INVALID);
            }
            for change in changes {
                index.entries.insert(
                    change.album_id,
                    Entry {
                        result: change.result,
                        online_complete: change.online_complete,
                        checked_at: now,
                    },
                );
            }
            write(path, &index)?;
            Ok(Value::Null)
        }
        _ => Err(INVALID),
    }
}
struct Message {
    request: Option<(String, Value)>,
    reply: mpsc::Sender<Result<Value>>,
}
#[derive(Clone)]
pub struct Controller {
    sender: mpsc::SyncSender<Message>,
    stopping: Arc<AtomicBool>,
}
impl Controller {
    pub fn new(path: PathBuf) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Message>(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stop = stopping.clone();
        std::thread::Builder::new()
            .name("sonante-windows-artwork-progress".into())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    let Some((operation, args)) = message.request else {
                        let _ = message.reply.send(Ok(Value::Null));
                        break;
                    };
                    let result = if worker_stop.load(Ordering::Acquire) {
                        Err("shutting_down")
                    } else {
                        now().and_then(|now| dispatch(&path, &operation, args, now))
                    };
                    if message.reply.send(result).is_err() {
                        eprintln!("Windows artwork progress: operation_receiver_closed");
                    }
                }
            })
            .map_err(|_| FAILED)?;
        Ok(Self { sender, stopping })
    }
    pub fn call(&self, operation: &str, args: Value) -> Result<Value> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        if !["list", "import", "save"].contains(&operation)
            || args
                .get("legacy")
                .and_then(Value::as_str)
                .is_some_and(|s| s.len() > MAX_BYTES)
            || args
                .get("changes")
                .and_then(Value::as_array)
                .is_some_and(|v| v.len() > 256)
        {
            return Err(INVALID);
        }
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message {
                request: Some((operation.into(), args)),
                reply,
            })
            .map_err(|_| BUSY)?;
        receiver.recv().map_err(|_| FAILED)?
    }
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message {
                request: None,
                reply,
            })
            .map_err(|_| BUSY)?;
        receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| FAILED)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0; 12];
            getrandom::fill(&mut random).unwrap();
            let dir = std::env::temp_dir().join(format!("sonante-art-progress-{random:x?}"));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> PathBuf {
            self.0.join("progress.json")
        }
        fn op(&self, operation: &str, args: Value, now: u64) -> Result<Value> {
            dispatch(&self.path(), operation, args, now)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn key(n: usize) -> String {
        format!("{n:064x}")
    }
    fn change(n: usize, result: &str, complete: bool) -> Value {
        json!({"album_id":key(n),"result":result,"online_complete":complete})
    }
    #[test]
    fn imports_once_converts_preview_milliseconds_without_storing_unknown_fields_or_art() {
        let f = Fixture::new();
        assert_eq!(f.op("list", json!({}), 100).unwrap(), Value::Null);
        let legacy=json!({key(1):{"album_id":key(1),"result":"local","online_complete":false,"checked_at":1700000000123u64,"thumb":"ignored-art"},key(2):{"result":"none","online_complete":true,"checked_at":1700000000}}).to_string();
        let view = f.op("import", json!({"legacy":legacy}), 100).unwrap();
        assert_eq!(view[key(1)]["checked_at"], 1700000000);
        assert_eq!(view[key(2)]["online_complete"], false);
        let disk = fs::read_to_string(f.path()).unwrap();
        assert!(!disk.contains("album_id"));
        assert!(!disk.contains("thumb"));
        assert!(!disk.contains("ignored-art"));
        assert_eq!(f.op("list", json!({}), 101).unwrap(), view);
        assert!(legacy.contains("1700000000123"));
    }
    #[test]
    fn original_batches_merge_in_seconds_and_late_legacy_never_overwrites_confirmed_native_changes()
    {
        let f = Fixture::new();
        f.op("import", json!({"legacy":"{}"}), 100).unwrap();
        f.op(
            "save",
            json!({"changes":[change(1,"local",false),change(2,"embedded",true)]}),
            101,
        )
        .unwrap();
        let first = fs::read(f.path()).unwrap();
        f.op("import", json!({"legacy":"invalid old input"}), 102)
            .unwrap();
        assert_eq!(fs::read(f.path()).unwrap(), first);
        f.op(
            "save",
            json!({"changes":[change(1,"none",false),change(3,"ineligible",true)]}),
            103,
        )
        .unwrap();
        let view = f.op("list", json!({}), 104).unwrap();
        assert_eq!(view[key(1)]["checked_at"], 103);
        assert_eq!(view[key(2)]["checked_at"], 101);
        assert_eq!(view[key(3)]["online_complete"], true);
        assert_eq!(view.as_object().unwrap().len(), 3);
    }
    #[test]
    fn unavailable_online_files_never_publish_positive_or_negative_completion_and_do_not_rewrite_disk(
    ) {
        let f = Fixture::new();
        f.op("import", json!({"legacy":"{}"}), 100).unwrap();
        f.op("save",json!({"changes":[change(1,"online",true),change(2,"none",true),change(3,"local",true),change(4,"embedded",true),change(5,"ineligible",true)]}),101).unwrap();
        let before = fs::read(f.path()).unwrap();
        let view = f.op("list", json!({}), 102).unwrap();
        for n in [1, 2] {
            assert_eq!(view[key(n)]["online_complete"], false);
        }
        for n in [3, 4, 5] {
            assert_eq!(view[key(n)]["online_complete"], true);
        }
        assert_eq!(fs::read(f.path()).unwrap(), before);
    }
    #[test]
    fn corrupt_future_invalid_entries_and_oversize_destination_are_preserved_not_empty_success() {
        let f = Fixture::new();
        for bytes in [b"broken".to_vec(),json!({"version":2,"entries":{}}).to_string().into_bytes(),json!({"version":1,"entries":{"bad":{"result":"none","online_complete":false,"checked_at":1}}}).to_string().into_bytes(),vec![b' ';MAX_BYTES+1]] {fs::write(f.path(),&bytes).unwrap();assert!(f.op("list",json!({}),100).is_err());assert!(f.op("import",json!({"legacy":"{}"}),100).is_err());assert!(f.op("save",json!({"changes":[]}),100).is_err());assert_eq!(fs::read(f.path()).unwrap(),bytes);}
    }
    #[test]
    fn whole_batch_is_validated_before_write_and_original_256_limit_is_enforced() {
        let f = Fixture::new();
        f.op("import", json!({"legacy":"{}"}), 100).unwrap();
        let before = fs::read(f.path()).unwrap();
        for changes in [
            vec![
                change(1, "local", false),
                json!({"album_id":"C:/private/path","result":"local","online_complete":false}),
            ],
            vec![change(1, "unknown", false)],
            vec![change(1, "local", false); 257],
        ] {
            assert!(f.op("save", json!({"changes":changes}), 101).is_err());
            assert_eq!(fs::read(f.path()).unwrap(), before);
        }
        f.op(
            "save",
            json!({"changes":(0..256).map(|n|change(n,"local",false)).collect::<Vec<_>>()}),
            101,
        )
        .unwrap();
        assert_eq!(
            f.op("list", json!({}), 102)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            256
        );
    }
    #[test]
    fn invalid_or_oversize_legacy_never_creates_a_store_and_nonfile_targets_are_untouched() {
        let f = Fixture::new();
        for legacy in [
            "{".to_string(),
            json!({"bad":{"result":"none","online_complete":false,"checked_at":1}}).to_string(),
            " ".repeat(MAX_BYTES + 1),
        ] {
            assert!(f.op("import", json!({"legacy":legacy}), 100).is_err());
            assert!(!f.path().exists());
        }
        fs::create_dir(f.path()).unwrap();
        fs::write(f.path().join("keep"), b"backup").unwrap();
        assert!(f.op("import", json!({"legacy":"{}"}), 100).is_err());
        assert_eq!(fs::read(f.path().join("keep")).unwrap(), b"backup");
    }
    #[test]
    fn file_lock_and_worker_shutdown_are_explicit_without_mutating_saved_data() {
        let f = Fixture::new();
        let c = Controller::new(f.path()).unwrap();
        c.call("import", json!({"legacy":"{}"})).unwrap();
        let before = fs::read(f.path()).unwrap();
        {
            let _held =
                crate::persistence::operation_lock(&f.path().with_extension("lock")).unwrap();
            assert_eq!(
                c.call("save", json!({"changes":[change(1,"local",false)]})),
                Err(BUSY)
            );
            assert_eq!(fs::read(f.path()).unwrap(), before);
        }
        assert_eq!(c.call("unknown", json!({})), Err(INVALID));
        c.shutdown().unwrap();
        assert_eq!(c.call("list", json!({})), Err("shutting_down"));
        assert_eq!(fs::read(f.path()).unwrap(), before);
    }
    #[test]
    fn separate_workers_merge_distinct_ids_under_the_existing_cross_instance_lock() {
        let f = Fixture::new();
        let seed = Controller::new(f.path()).unwrap();
        seed.call("import", json!({"legacy":"{}"})).unwrap();
        let jobs = (1..=6)
            .map(|n| {
                let path = f.path();
                std::thread::spawn(move || {
                    let c = Controller::new(path).unwrap();
                    let deadline = std::time::Instant::now() + Duration::from_secs(3);
                    loop {
                        match c.call("save", json!({"changes":[change(n,"local",false)]})) {
                            Ok(_) => break,
                            Err(BUSY) if std::time::Instant::now() < deadline => {
                                std::thread::yield_now()
                            }
                            other => panic!("unexpected {other:?}"),
                        }
                    }
                    c.shutdown().unwrap();
                })
            })
            .collect::<Vec<_>>();
        for job in jobs {
            job.join().unwrap();
        }
        assert_eq!(
            seed.call("list", json!({}))
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            6
        );
        seed.shutdown().unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn reparse_surrogate_is_refused_without_touching_the_pointed_file() {
        let f = Fixture::new();
        let target = f.0.join("keep");
        fs::write(&target, b"backup").unwrap();
        std::os::unix::fs::symlink(&target, f.path()).unwrap();
        assert!(f.op("list", json!({}), 100).is_err());
        assert!(f.op("import", json!({"legacy":"{}"}), 100).is_err());
        assert_eq!(fs::read(target).unwrap(), b"backup");
    }
}
