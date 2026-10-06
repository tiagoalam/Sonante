use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static PROGRESS_LOCK: Mutex<()> = Mutex::new(());
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Local,
    Embedded,
    Online,
    Ineligible,
    None,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub result: ResultKind,
    pub online_complete: bool,
    pub checked_at: u64,
}

#[derive(Deserialize, Serialize)]
struct Index {
    version: u8,
    entries: BTreeMap<String, Entry>,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
pub struct Change {
    pub album_id: String,
    pub result: ResultKind,
    pub online_complete: bool,
}

fn load(dir: &Path) -> Index {
    let path = dir.join("enrichment-progress.json");
    if let Err(error) =
        crate::persistence::prepare_private_file_for_load(&path, "progresso de capas")
    {
        eprintln!("Progresso de capas indisponível: {error}");
        return Index::default();
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Index::default(),
        Err(error) => {
            eprintln!("Falha ao ler progresso de capas: {error}");
            return Index::default();
        }
    };
    match serde_json::from_slice::<Index>(&bytes) {
        Ok(index) if index.version == VERSION => index,
        _ => {
            eprintln!("Progresso de capas inválido; reconstruindo apenas esse estado descartável.");
            Index::default()
        }
    }
}

fn now_secs() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Relógio do sistema inválido.".to_string())?
        .as_secs())
}

fn valid_entries(index: Index, online: &BTreeMap<String, &'static str>) -> BTreeMap<String, Entry> {
    index
        .entries
        .into_iter()
        .map(|(id, mut entry)| {
            if entry.online_complete {
                let valid = match entry.result {
                    ResultKind::Online => online.get(&id) == Some(&"positive"),
                    ResultKind::None => online.get(&id) == Some(&"negative"),
                    ResultKind::Local | ResultKind::Embedded | ResultKind::Ineligible => true,
                };
                if !valid {
                    entry.online_complete = false;
                }
            }
            (id, entry)
        })
        .collect()
}

pub fn read_at(dir: &Path, now: u64) -> BTreeMap<String, Entry> {
    let _guard = match PROGRESS_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            eprintln!("Progresso de capas indisponível: lock corrompido.");
            return BTreeMap::new();
        }
    };
    let online = match crate::online_artwork::cache_entry_statuses(dir, now) {
        Ok(statuses) => statuses,
        Err(error) => {
            eprintln!("Estado do cache online indisponível: {error}");
            BTreeMap::new()
        }
    };
    valid_entries(load(dir), &online)
}

pub fn write_at(dir: &Path, changes: Vec<Change>, now: u64) -> Result<(), String> {
    if changes.len() > 256 {
        return Err("Lote de progresso de capas excede o limite.".into());
    }
    let _guard = PROGRESS_LOCK
        .lock()
        .map_err(|_| "Progresso de capas indisponível.".to_string())?;
    let mut index = load(dir);
    for change in changes {
        if change.album_id.is_empty() || change.album_id.len() > 4096 {
            return Err("Identificador de álbum inválido no progresso de capas.".into());
        }
        index.entries.insert(
            change.album_id,
            Entry {
                result: change.result,
                online_complete: change.online_complete,
                checked_at: now,
            },
        );
    }
    let bytes = serde_json::to_vec(&index)
        .map_err(|error| format!("Falha ao serializar progresso de capas: {error}"))?;
    crate::persistence::atomic_write_private(
        &dir.join("enrichment-progress.json"),
        &bytes,
        "progresso de capas",
    )
}

pub async fn read() -> Result<BTreeMap<String, Entry>, String> {
    let dir = crate::online_artwork::cache_dir()?;
    let now = now_secs()?;
    tauri::async_runtime::spawn_blocking(move || read_at(&dir, now))
        .await
        .map_err(|error| format!("Falha na tarefa de progresso de capas: {error}"))
}

pub async fn write(changes: Vec<Change>) -> Result<(), String> {
    let dir = crate::online_artwork::cache_dir()?;
    let now = now_secs()?;
    tauri::async_runtime::spawn_blocking(move || write_at(&dir, changes, now))
        .await
        .map_err(|error| format!("Falha na tarefa de progresso de capas: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "sonante-artwork-progress-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn persisted_local_and_embedded_survive_restart() {
        let dir = test_dir();
        write_at(
            &dir,
            vec![
                Change {
                    album_id: "local".into(),
                    result: ResultKind::Local,
                    online_complete: false,
                },
                Change {
                    album_id: "embedded".into(),
                    result: ResultKind::Embedded,
                    online_complete: false,
                },
            ],
            100,
        )
        .unwrap();
        let entries = read_at(&dir, 101);
        assert_eq!(entries["local"].result, ResultKind::Local);
        assert_eq!(entries["embedded"].result, ResultKind::Embedded);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_progress_does_not_touch_online_index() {
        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("enrichment-progress.json"), b"broken").unwrap();
        fs::write(dir.join("index.json"), b"online-index").unwrap();
        assert!(read_at(&dir, 100).is_empty());
        assert_eq!(fs::read(dir.join("index.json")).unwrap(), b"online-index");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn online_progress_follows_existing_positive_and_negative_cache() {
        let index = Index {
            version: VERSION,
            entries: [
                (
                    "positive".into(),
                    Entry {
                        result: ResultKind::Online,
                        online_complete: true,
                        checked_at: 100,
                    },
                ),
                (
                    "negative".into(),
                    Entry {
                        result: ResultKind::None,
                        online_complete: true,
                        checked_at: 100,
                    },
                ),
                (
                    "expired".into(),
                    Entry {
                        result: ResultKind::None,
                        online_complete: true,
                        checked_at: 100,
                    },
                ),
            ]
            .into(),
        };
        let online = [
            ("positive".into(), "positive"),
            ("negative".into(), "negative"),
        ]
        .into();
        let entries = valid_entries(index, &online);
        assert!(entries["positive"].online_complete);
        assert!(entries["negative"].online_complete);
        assert!(!entries["expired"].online_complete);
    }

    #[test]
    fn unknown_progress_version_is_discarded_without_touching_online_index() {
        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("enrichment-progress.json"),
            br#"{"version":99,"entries":{}}"#,
        )
        .unwrap();
        assert!(read_at(&dir, 100).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn negative_cache_expiry_and_missing_positive_file_reopen_global_work() {
        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("index.json"),
            br#"{"version":1,"entries":{"negative":{"status":"negative","checked_at":100},"positive":{"status":"positive","file":"missing.img","source":"cover_art_archive","musicbrainz_release_group_id":"id","mime":"image/jpeg","checked_at":100}}}"#,
        ).unwrap();
        write_at(
            &dir,
            vec![
                Change {
                    album_id: "negative".into(),
                    result: ResultKind::None,
                    online_complete: true,
                },
                Change {
                    album_id: "positive".into(),
                    result: ResultKind::Online,
                    online_complete: true,
                },
            ],
            100,
        )
        .unwrap();
        let fresh = read_at(&dir, 101);
        assert!(fresh["negative"].online_complete);
        assert!(!fresh["positive"].online_complete);
        let expired = read_at(&dir, 100 + 7 * 24 * 60 * 60);
        assert!(!expired["negative"].online_complete);
        fs::remove_dir_all(dir).unwrap();
    }
}
