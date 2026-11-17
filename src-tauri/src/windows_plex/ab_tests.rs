use super::*;
use crate::windows_shared::{ab_comparison as ab, ab_lifecycle as life};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
fn wave() -> Vec<u8> {
    let n = 5 * 48000 * 4;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + n as u32).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(48000u32.to_le_bytes());
    b.extend(192000u32.to_le_bytes());
    b.extend(4u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((n as u32).to_le_bytes());
    b.resize(44 + n, 0);
    b
}
struct Fixture {
    host: Controller,
    binding: Arc<Binding>,
    base: PathBuf,
    mode: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
    entered: Arc<tokio::sync::Notify>,
    stop: tokio::sync::watch::Sender<bool>,
    peer: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = paths.clone();
        let mode = Arc::new(AtomicUsize::new(0));
        let state = mode.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let peer = tokio::spawn(async move {
            loop {
                let (mut socket, _) =
                    tokio::select! {_=stopped.changed()=>break,v=listener.accept()=>v.unwrap()};
                let mut bytes = Vec::new();
                loop {
                    let Ok(b) = socket.read_u8().await else {
                        break;
                    };
                    bytes.push(b);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    assert!(bytes.len() < 8192);
                }
                let request = String::from_utf8(bytes).unwrap();
                let target = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let url = reqwest::Url::parse(&format!("http://{address}{target}")).unwrap();
                let path = url.path();
                seen.lock().unwrap().push(path.into());
                let m = state.load(Ordering::Acquire);
                let body=match path{
    "/identity"=>serde_json::json!({"MediaContainer":{"machineIdentifier":"server-one"}}).to_string().into_bytes(),
    "/library/sections"=>serde_json::json!({"MediaContainer":{"Directory":[{"key":"7","type":"artist","title":"Own"}]}}).to_string().into_bytes(),
    "/library/metadata/album-one/children"=>serde_json::json!({"MediaContainer":{"librarySectionID":7,"size":2,"totalSize":if m==1{3}else{2},"offset":0,"Metadata":([1,2].map(|n|serde_json::json!({"type":"track","ratingKey":format!("t{n}"),"parentRatingKey":"album-one","title":if m==6{"FIXTURE_server-one".into()}else{format!("Own {n}")},"duration":5000,"Media":[{"Part":[{"key":format!("/library/parts/{n}/file.wav"),"file":format!("/private/own/{n}.wav"),"size":960044}]}]})))}}).to_string().into_bytes(),
    "/library/parts/1/file.wav"|"/library/parts/2/file.wav"=>wave(),
    other=>panic!("unexpected own fixture path {other}"),
   };
                let media = path.starts_with("/library/parts/");
                let code = if media && m == 5 {
                    "401 Unauthorized"
                } else {
                    "200 OK"
                };
                let kind = if media && m != 2 {
                    "audio/wav"
                } else {
                    "application/json"
                };
                let length = if media && m == 3 {
                    ab::MAX_SOURCE_BYTES + 1
                } else {
                    body.len()
                };
                let _=socket.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: {kind}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").as_bytes()).await;
                if media && m == 4 {
                    signal.notify_one();
                    let _ = stopped.changed().await;
                    break;
                }
                let _ = socket.write_all(&body).await;
            }
        });
        let host = host(Arc::new(MemoryVault::default()));
        let mut a = account("server-one");
        a.server_url = format!("http://{address}");
        host.configure(true, Some(PreparedAccount(Some(a))))
            .await
            .unwrap();
        let settings = host.settings().await.unwrap();
        let mut binding = Binding::new(settings.clone()).unwrap();
        let fresh = Arc::get_mut(&mut binding).unwrap();
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let (tx, _) = tokio::sync::watch::channel(false);
        fresh.client = Client {
            manager: Arc::new(crate::windows_plex::core::PlexConnectionManager::fixture(
                &settings,
                http.clone(),
            )),
            network: Arc::new(tokio::sync::Semaphore::new(4)),
            artwork: Arc::new(tokio::sync::Semaphore::new(2)),
            stop: tx,
        };
        fresh.stop = fresh.client.stop.clone();
        fresh.stream_http = http;
        host.registry()
            .replace(Some(binding.clone()), || Ok(()))
            .unwrap();
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).unwrap();
        let base = std::env::temp_dir().join(format!("sonante-own-ab-plex-{random:x?}"));
        std::fs::create_dir(&base).unwrap();
        Self {
            host,
            binding,
            base,
            mode,
            paths,
            entered,
            stop,
            peer,
        }
    }
    async fn tracks(&self) -> Vec<ab::Track> {
        self.host
            .ab_tracks("server-one", "7", "album-one", Arc::new(|| true))
            .await
            .unwrap()
    }
    async fn input(&self, key: String, current: ab::Check) -> Result<ab::Input, String> {
        let host = self.host.clone();
        let base = self.base.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime.block_on(host.ab_input("server-one", "7", "album-one", &key, &base, current))
        })
        .await
        .unwrap()
    }
    async fn close(self) {
        assert!(
            self.binding.port.lock().unwrap().is_none(),
            "A/B never binds the MPD proxy"
        );
        self.host.shutdown().unwrap();
        self.stop.send_replace(true);
        self.peer.await.unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while std::fs::read_dir(&self.base).unwrap().count() != 0 {
            assert!(
                Instant::now() < end,
                "all private spools physically retired"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        std::fs::remove_dir(&self.base).unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_plex_inventory_private_spool_original_decode_and_retirement() {
    let _serial = crate::windows_shared::AB_TEST_SERIAL.lock().unwrap();
    let f = Fixture::new().await;
    let token = life::begin(Arc::new(|| true)).unwrap();
    let tracks = f.tracks().await;
    assert_eq!(tracks.len(), 2);
    let wire = serde_json::to_string(&tracks).unwrap();
    assert!(
        !wire.contains("/private") && !wire.contains("FIXTURE_") && !wire.contains("library/parts")
    );
    let job = ab::job(&token).unwrap();
    for (i, side) in [ab::Side::A, ab::Side::B].into_iter().enumerate() {
        let input = f
            .input(tracks[i].key.clone(), life::policy(&token).unwrap())
            .await
            .unwrap();
        let j = job.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            j.install(side, input).unwrap();
            ab::decode(j, side, 0., 30).unwrap()
        })
        .await
        .unwrap();
        assert_eq!(&bytes[..4], b"SAB1");
        assert!(bytes.len() > 48000 * 4 * 2 * 3);
    }
    assert!(job.valid());
    f.host.apply_enabled_from_config(false).await.unwrap();
    assert!(!job.valid());
    life::cancel(&token);
    f.close().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plex_rejects_incomplete_foreign_and_credential_shaped_inventory_before_media() {
    let f = Fixture::new().await;
    assert!(f
        .host
        .ab_tracks("foreign", "7", "album-one", Arc::new(|| true))
        .await
        .is_err());
    assert!(f.paths.lock().unwrap().is_empty());
    for mode in [1, 6] {
        f.mode.store(mode, Ordering::Release);
        assert!(f
            .host
            .ab_tracks("server-one", "7", "album-one", Arc::new(|| true))
            .await
            .is_err());
    }
    assert!(!f
        .paths
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.starts_with("/library/parts/")));
    f.close().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plex_body_limits_content_auth_and_unknown_key_leave_no_spool_or_route_retry() {
    let f = Fixture::new().await;
    let tracks = f.tracks().await;
    assert_eq!(
        f.input("f".repeat(64), Arc::new(|| true))
            .await
            .err()
            .as_deref(),
        Some("AB_CHANGED")
    );
    for mode in [2, 3, 5] {
        f.mode.store(mode, Ordering::Release);
        let before = f
            .paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with("/library/parts/"))
            .count();
        assert!(f
            .input(tracks[0].key.clone(), Arc::new(|| true))
            .await
            .is_err());
        let after = f
            .paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with("/library/parts/"))
            .count();
        assert_eq!(after, before + 1);
        assert_eq!(std::fs::read_dir(&f.base).unwrap().count(), 0);
    }
    f.close().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_stalled_download_returns_without_waiting_for_body_timeout() {
    let f = Fixture::new().await;
    let tracks = f.tracks().await;
    f.mode.store(4, Ordering::Release);
    let live = Arc::new(AtomicBool::new(true));
    let l = live.clone();
    let check: ab::Check = Arc::new(move || l.load(Ordering::Acquire));
    let host = f.host.clone();
    let base = f.base.clone();
    let key = tracks[0].key.clone();
    let runtime = tokio::runtime::Handle::current();
    let worker = tokio::task::spawn_blocking(move || {
        runtime.block_on(host.ab_input("server-one", "7", "album-one", &key, &base, check))
    });
    tokio::time::timeout(Duration::from_secs(3), f.entered.notified())
        .await
        .unwrap();
    live.store(false, Ordering::Release);
    let result = tokio::time::timeout(Duration::from_millis(700), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.err().as_deref(), Some("AB_CHANGED"));
    f.close().await;
}
