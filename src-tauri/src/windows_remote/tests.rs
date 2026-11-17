use crate::{
    remote::{Command, Snapshot},
    windows_mpd::library::Track,
    windows_remote::model as m,
};
fn track() -> Track {
    serde_json::from_value(serde_json::json!({"id":"a","title":"Song","artist":"Artist","album":"Album","disc":1,"number":1,"fileName":"1.wav","catalogAlbumId":"album","path":"PRIVATE_PATH"})).unwrap()
}
fn view(rows: &[Track], index: usize, rev: &str, direct: bool) -> m::Frame {
    m::frame(
        7, false, "stop", "pause", 4., 100., 50, "", rows, index, rev, true, false, direct, false,
    )
}
#[test]
fn remote_queue_and_media_bind_occurrences_and_never_expose_private_locations() {
    let t = track();
    let rows = vec![t.clone(), t.clone()];
    let a = view(&rows, 0, "wq1:7:logical", false);
    let b = view(&rows, 1, "wq1:7:logical", false);
    assert_ne!(a.snapshot.media, b.snapshot.media);
    let text = serde_json::to_string(&a.snapshot).unwrap();
    assert!(!text.contains("PRIVATE_PATH"));
    assert!(!text.contains("windows/"));
    assert_eq!(a.snapshot.state, "pause");
    let page = m::queue(&a, &rows, &[Some(10.), Some(20.)], 0).unwrap();
    assert_eq!(page.tracks[1].duration, Some(20.));
    assert!(page.revision.starts_with("q1:"));
    assert_eq!(page.revision.len(), 67);
    assert!(Command::QueuePlay {
        index: 1,
        revision: page.revision.clone()
    }
    .valid());
    assert!(m::resolve(
        &Command::Play {
            media: b.snapshot.media
        },
        &a
    )
    .is_err());
    assert!(m::resolve(
        &Command::QueuePlay {
            index: 1,
            revision: page.revision
        },
        &a
    )
    .is_ok());
}
#[test]
fn remote_revision_cannot_survive_native_queue_change_or_generation_change() {
    let rows = vec![track()];
    let a = view(&rows, 0, "wq1:7:logical", false);
    for revision in ["wq1:8:logical", "wq1:7:2"] {
        let changed = view(&rows, 0, revision, false);
        assert!(m::resolve(
            &Command::QueuePlay {
                index: 0,
                revision: a.revision.clone()
            },
            &changed
        )
        .is_err());
    }
}
#[test]
fn remote_volume_cannot_bypass_direct_and_missing_metadata_is_unknown() {
    let rows = vec![track()];
    let direct = view(&rows, 0, "wq1:7:logical", true);
    assert!(!direct.snapshot.volume_writable);
    assert!(m::resolve(&Command::Volume { value: 99 }, &direct).is_err());
    let a = view(&rows, 0, "wq1:7:logical", false);
    assert!(m::resolve(&Command::Volume { value: 99 }, &a).is_ok());
    let page = m::queue(&a, &rows, &[], 0).unwrap();
    assert_eq!(page.tracks[0].duration, None);
    assert!(m::queue(&a, &rows, &[], 2).is_err());
}
#[test]
fn remote_invalid_or_unconfirmed_snapshots_never_authorize_transport() {
    let rows = vec![track()];
    let mut a = view(&rows, 0, "wq1:7:logical", false);
    a.snapshot.available = false;
    assert!(m::resolve(
        &Command::Play {
            media: a.snapshot.media.clone()
        },
        &a
    )
    .is_err());
    a.snapshot = Snapshot::default();
    assert!(m::resolve(&Command::Volume { value: 50 }, &a).is_err());
}
#[test]
fn actual_client_store_preserves_corruption_and_does_not_resurrect_or_reset() {
    let dir = std::env::temp_dir().join(format!("sonante-remote-new-{}", std::process::id()));
    let _ = std::fs::remove_dir(&dir);
    std::fs::create_dir(&dir).unwrap();
    let p = dir.join("clients");
    crate::windows_remote::storage::write(&p, b"bad json").unwrap();
    let bytes = std::fs::read(&p).unwrap();
    assert!(crate::windows_remote::storage::read(&p, 3).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    std::fs::remove_file(p).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn actual_http_wire_uses_original_auth_and_windows_bindings_without_audio_or_external_server() {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };
    struct Adapter {
        rows: Vec<Track>,
        revision: Mutex<String>,
        calls: AtomicUsize,
        active: AtomicBool,
    }
    impl Adapter {
        fn frame(&self) -> m::Frame {
            view(&self.rows, 0, &self.revision.lock().unwrap(), false)
        }
    }
    impl crate::remote::Backend for Adapter {
        fn active(&self) -> bool {
            self.active.load(Ordering::Acquire)
        }
        fn queue(&self, o: usize) -> Result<crate::remote::QueuePage, String> {
            m::queue(&self.frame(), &self.rows, &[Some(100.)], o)
        }
        fn control(&self, c: Command, guard: crate::remote::Guard) -> Result<(), String> {
            let f = self.frame();
            if !guard() {
                return Err("REMOTE_CHANGED".into());
            }
            m::resolve(&c, &f).map_err(str::to_owned)?;
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
        fn artwork(
            &self,
            _: &str,
            _: crate::remote::Guard,
        ) -> Result<Option<(String, Vec<u8>)>, String> {
            Ok(None)
        }
        fn queue_artwork(
            &self,
            _: &str,
            _: crate::remote::Guard,
        ) -> Result<Option<(String, Vec<u8>)>, String> {
            Ok(None)
        }
    }
    let backend = Arc::new(Adapter {
        rows: vec![track()],
        revision: Mutex::new("wq1:7:logical".into()),
        calls: AtomicUsize::new(0),
        active: AtomicBool::new(true),
    });
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(crate::remote::Shared::default());
    *shared.snapshot.lock().unwrap() = Some(crate::remote::Stamped {
        snapshot: backend.frame().snapshot,
        at: std::time::Instant::now(),
    });
    let bearer = shared.auth.lock().unwrap().test_token();
    let token = bearer.clone();
    let active = stop.clone();
    let state = shared.clone();
    let app = backend.clone();
    let join = std::thread::spawn(move || crate::remote_http::serve(listener, active, state, app));
    struct Worker(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
    impl Drop for Worker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
            self.1.take().unwrap().join().unwrap();
        }
    }
    let _worker = Worker(stop, Some(join));
    let request =
        |method: &str, path: &str, body: Option<serde_json::Value>, auth: bool| -> String {
            let bytes = body
                .map(|v| serde_json::to_vec(&v).unwrap())
                .unwrap_or_default();
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let headers = format!(
                "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{}{}\r\n",
                if auth {
                    format!("Authorization: Bearer {token}\r\n")
                } else {
                    String::new()
                },
                if method == "POST" {
                    format!(
                        "Content-Type: application/json\r\nContent-Length: {}\r\n",
                        bytes.len()
                    )
                } else {
                    String::new()
                }
            );
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(&bytes).unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        };
    let unauth = request("GET", "/api/v1/state", None, false);
    assert!(unauth.starts_with("HTTP/1.1 401"));
    let response = request("GET", "/api/v1/queue?offset=0", None, true);
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(!response.contains("PRIVATE_PATH"));
    let body: serde_json::Value =
        serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let rev = body["revision"].as_str().unwrap();
    let command = serde_json::json!({"request_id":"own-request-0001","command":{"action":"queue_play","index":0,"revision":rev}});
    for _ in 0..2 {
        let response = request("POST", "/api/v1/control", Some(command.clone()), true);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }
    assert_eq!(backend.calls.load(Ordering::Acquire), 1);
    *backend.revision.lock().unwrap() = "wq1:7:2".into();
    let stale = serde_json::json!({"request_id":"own-request-0002","command":{"action":"queue_play","index":0,"revision":rev}});
    let rejected = request("POST", "/api/v1/control", Some(stale), true);
    assert!(rejected.starts_with("HTTP/1.1 409"));
    assert!(rejected.contains("changed"));
    assert_eq!(backend.calls.load(Ordering::Acquire), 1);
    shared.revoke().unwrap();
    assert!(request("GET", "/api/v1/queue?offset=0", None, true).starts_with("HTTP/1.1 401"));
    assert_eq!(backend.calls.load(Ordering::Acquire), 1);
}

#[test]
fn disabling_without_an_active_worker_never_repairs_corrupt_saved_grants() {
    use std::{
        path::PathBuf,
        sync::{atomic::Ordering, Arc},
    };
    struct Off(PathBuf);
    impl crate::remote::Backend for Off {
        fn active(&self) -> bool {
            false
        }
        fn credential_path(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
        fn queue(&self, _: usize) -> Result<crate::remote::QueuePage, String> {
            panic!("no backend reads")
        }
        fn control(&self, _: Command, _: crate::remote::Guard) -> Result<(), String> {
            panic!("no playback")
        }
        fn artwork(
            &self,
            _: &str,
            _: crate::remote::Guard,
        ) -> Result<Option<(String, Vec<u8>)>, String> {
            panic!("no media")
        }
        fn queue_artwork(
            &self,
            _: &str,
            _: crate::remote::Guard,
        ) -> Result<Option<(String, Vec<u8>)>, String> {
            panic!("no media")
        }
    }
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).unwrap();
    let dir = std::env::temp_dir().join(format!("sonante-remote-corruption-{nonce:x?}"));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("clients.json");
    crate::windows_remote::storage::write(&path, b"corrupt own record").unwrap();
    let state = crate::remote::State::default();
    assert!(state
        .configure(
            crate::remote::Preferences {
                enabled: false,
                port: 8787
            },
            Arc::new(Off(path.clone()))
        )
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt own record");
    assert!(!state.enabled.load(Ordering::Acquire));
    assert!(!state.status(vec![]).unwrap().enabled);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
