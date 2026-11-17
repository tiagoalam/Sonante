use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
fn owner() -> Result<(), String> {
    Ok(())
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .build()
        .map_err(|_| UNAVAILABLE.into())
}
fn path() -> PathBuf {
    std::env::temp_dir()
        .join(format!("sonante-nav-host-{}", api::random().unwrap()))
        .join("account.json")
}
fn credentials() -> Credentials {
    Credentials {
        connection_id: "OLD-SAVED".into(),
        server: "http://127.0.0.1:9/".into(),
        username: "fixture".into(),
        password: "PRIVATE-fixture".into(),
    }
}
#[derive(Default)]
struct MemoryVault {
    saved: Mutex<Option<Credentials>>,
    loads: AtomicUsize,
    writes: AtomicUsize,
    error: Mutex<Option<String>>,
    pause: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}
impl Vault for MemoryVault {
    fn load(&self, _: &Path) -> Result<Option<Credentials>, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.error.lock().unwrap().as_ref() {
            return Err(e.clone());
        }
        Ok(self.saved.lock().unwrap().clone())
    }
    fn save(&self, _: &Path, value: Option<Credentials>) -> Result<(), String> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.error.lock().unwrap().as_ref() {
            return Err(e.clone());
        }
        let pause = self.pause.lock().unwrap().take();
        if let Some((start, release)) = pause {
            start.send(()).unwrap();
            release.recv().unwrap();
        }
        *self.saved.lock().unwrap() = value;
        Ok(())
    }
}
async fn ping(status: &str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(socket.read_u8().await.unwrap());
            assert!(header.len() < 8192);
        }
        let header = String::from_utf8(header).unwrap();
        assert!(header.starts_with("GET /rest/ping.view?"));
        assert!(!header.contains("PRIVATE-fixture"));
        let body = r#"{"subsonic-response":{"status":"ok"}}"#;
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
    });
    (format!("http://{address}/"), peer)
}
async fn drain(controller: &Controller) {
    controller.begin_shutdown();
    let c = controller.clone();
    tokio::task::spawn_blocking(move || c.shutdown())
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_owner_is_required_before_any_account_file_or_network_and_construction_is_quiet() {
    let path = path();
    let controller = Controller::new(path.clone()).unwrap();
    assert!(!path.parent().unwrap().exists());
    assert!(controller.sender.lock().unwrap().is_none());
    assert!(matches!(controller.status().await,Err(e)if e==STORAGE));
    assert!(!path.parent().unwrap().exists());
    drain(&controller).await;
    assert!(controller.status().await.is_err());
    assert!(!path.parent().unwrap().exists());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lazy_account_restore_uses_fresh_generation_without_media_listener_or_autoplay() {
    let vault = Arc::new(MemoryVault::default());
    *vault.saved.lock().unwrap() = Some(credentials());
    let path = path();
    let c = Controller::with_parts(path.clone(), vault.clone(), owner, client).unwrap();
    let saved = c.status().await.unwrap();
    assert!(saved.connection_id.is_none());
    assert_eq!(saved.username.as_deref(), Some("fixture"));
    assert_eq!(vault.loads.load(Ordering::SeqCst), 1);
    let active = c.apply_enabled_from_config(true).await.unwrap();
    assert_ne!(active.connection_id.as_deref(), Some("OLD-SAVED"));
    assert!(active.account_id.is_some());
    let connection = c.registry().selected().unwrap().unwrap();
    assert!(!connection.playback_listener_started());
    assert_eq!(
        c.status().await.unwrap().connection_id,
        active.connection_id
    );
    assert_eq!(vault.loads.load(Ordering::SeqCst), 2);
    c.apply_enabled_from_config(false).await.unwrap();
    assert!(!connection.active());
    assert!(vault.saved.lock().unwrap().is_some());
    assert_eq!(vault.writes.load(Ordering::SeqCst), 0);
    assert!(!path.parent().unwrap().exists());
    drain(&c).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_awaiter_keeps_serial_write_until_the_later_disconnect_commits() {
    let vault = Arc::new(MemoryVault::default());
    let (start, started) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *vault.pause.lock().unwrap() = Some((start, wait));
    let c = Controller::with_parts(path(), vault.clone(), owner, client).unwrap();
    c.apply_enabled_from_config(true).await.unwrap();
    let (server, peer) = ping("200 OK").await;
    let actor = c.clone();
    let connect = tokio::spawn(async move {
        actor
            .connect(server, "fixture".into(), "PRIVATE-fixture".into())
            .await
    });
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    connect.abort();
    assert!(connect.await.is_err_and(|error| error.is_cancelled()));
    let actor = c.clone();
    let disconnect = tokio::spawn(async move { actor.disconnect().await });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if c.context.gate.state.lock().unwrap().1 == 2 {
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    assert_eq!(vault.writes.load(Ordering::SeqCst), 1);
    assert_eq!(c.context.gate.state.lock().unwrap().1, 2);
    release.send(()).unwrap();
    disconnect.await.unwrap().unwrap();
    peer.await.unwrap();
    assert_eq!(vault.writes.load(Ordering::SeqCst), 2);
    assert!(vault.saved.lock().unwrap().is_none());
    assert!(c.registry().selected().unwrap().is_none());
    drain(&c).await;
    assert_eq!(c.context.gate.state.lock().unwrap().1, 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authentication_and_store_failures_keep_the_previous_account_and_live_context() {
    let vault = Arc::new(MemoryVault::default());
    *vault.saved.lock().unwrap() = Some(credentials());
    let c = Controller::with_parts(path(), vault.clone(), owner, client).unwrap();
    let before = c.apply_enabled_from_config(true).await.unwrap();
    let (server, peer) = ping("401 Unauthorized").await;
    assert!(
        matches!(c.connect(server,"new".into(),"PRIVATE-fixture".into()).await,Err(e)if e==super::super::AUTH)
    );
    peer.await.unwrap();
    assert_eq!(vault.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.status().await.unwrap().connection_id,
        before.connection_id
    );
    *vault.error.lock().unwrap() = Some(STORAGE.into());
    let (server, peer) = ping("200 OK").await;
    assert!(
        matches!(c.connect(server,"new".into(),"PRIVATE-fixture".into()).await,Err(e)if e==STORAGE)
    );
    peer.await.unwrap();
    assert_eq!(
        c.status().await.unwrap().connection_id,
        before.connection_id
    );
    assert!(matches!(c.disconnect().await,Err(e)if e==STORAGE));
    assert_eq!(
        c.status().await.unwrap().connection_id,
        before.connection_id
    );
    assert_eq!(
        vault.saved.lock().unwrap().as_ref().unwrap().username,
        "fixture"
    );
    drain(&c).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_closes_admission_and_skips_queued_writes_without_releasing_a_running_file_lease()
{
    let vault = Arc::new(MemoryVault::default());
    let (start, started) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *vault.pause.lock().unwrap() = Some((start, wait));
    let c = Controller::with_parts(path(), vault.clone(), owner, client).unwrap();
    let (server, peer) = ping("200 OK").await;
    let a = c.clone();
    let running = tokio::spawn(async move {
        a.connect(server, "fixture".into(), "PRIVATE-fixture".into())
            .await
    });
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut pending = Vec::new();
    for _ in 0..15 {
        let a = c.clone();
        pending.push(tokio::spawn(async move { a.disconnect().await }));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if c.context.gate.state.lock().unwrap().1 == 16 {
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    assert!(matches!(c.status().await,Err(e)if e==UNAVAILABLE));
    c.begin_shutdown();
    assert!(matches!(c.status().await,Err(e)if e==CHANGED));
    assert_eq!(c.context.gate.state.lock().unwrap().1, 16);
    assert_eq!(vault.writes.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    assert!(running.await.unwrap().is_err());
    for p in pending {
        assert!(p.await.unwrap().is_err());
    }
    peer.await.unwrap();
    drain(&c).await;
    assert_eq!(vault.writes.load(Ordering::SeqCst), 1);
    assert_eq!(c.context.gate.state.lock().unwrap().1, 0);
    assert!(c.registry().selected().unwrap().is_none());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_capacity_and_shutdown_cancel_pending_futures_without_starting_the_account_worker() {
    let c =
        Controller::with_parts(path(), Arc::new(MemoryVault::default()), owner, client).unwrap();
    let permits: Vec<_> = (0..4)
        .map(|_| c.context.network.clone().try_acquire_owned().unwrap())
        .collect();
    assert!(matches!(c.read(async{Ok::<_,String>(())}).await,Err(e)if e==UNAVAILABLE));
    drop(permits);
    let a = c.clone();
    let (start, started) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn(async move {
        a.read(async move {
            start.send(()).unwrap();
            std::future::pending::<Result<(), String>>().await
        })
        .await
    });
    started.await.unwrap();
    c.begin_shutdown();
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(1),pending).await.unwrap().unwrap(),Err(e)if e==CHANGED)
    );
    assert!(c.sender.lock().unwrap().is_none());
    drain(&c).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_album_and_favorite_flow_keeps_metadata_durable_without_starting_playback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(socket.read_u8().await.unwrap());
                assert!(bytes.len() < 8192);
            }
            let request = String::from_utf8(bytes).unwrap();
            let body = if request.starts_with("GET /rest/ping.view?") {
                serde_json::json!({"subsonic-response":{"status":"ok"}})
            } else {
                assert!(request.starts_with("GET /rest/getAlbum.view?"));
                serde_json::json!({"subsonic-response":{"status":"ok","album":{"id":"album","name":"Fixture Album","artist":"Fixture Artist","coverArt":"cover","song":[{"id":"song","title":"Fixture Song"}]}}})
            };
            let body = body.to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let path = path();
    let c = Controller::with_parts(
        path.clone(),
        Arc::new(MemoryVault::default()),
        owner,
        client,
    )
    .unwrap();
    c.apply_enabled_from_config(true).await.unwrap();
    let status = c
        .connect(
            format!("http://{address}/"),
            "fixture".into(),
            "PRIVATE-fixture".into(),
        )
        .await
        .unwrap();
    let id = status.connection_id.unwrap();
    let favorites = c
        .favorites(id.clone(), Some("album".into()), Some(true))
        .await
        .unwrap();
    assert_eq!(favorites[0].cover_id.as_deref(), Some("cover"));
    peer.await.unwrap();
    assert_eq!(
        c.favorites(id.clone(), None, None).await.unwrap()[0].id,
        "album"
    );
    assert!(!c
        .registry()
        .selected()
        .unwrap()
        .unwrap()
        .playback_listener_started());
    assert_eq!(c.registry().prepared(&id, "album").unwrap().songs.len(), 1);
    let bytes = std::fs::read_to_string(path.with_file_name("windows-navidrome-favorites-v1.json"))
        .unwrap();
    assert!(!bytes.contains("PRIVATE") && !bytes.contains("127.0.0.1"));
    // Removal remains available after the synthetic server has already closed.
    assert!(c
        .favorites(id, Some("album".into()), Some(false))
        .await
        .unwrap()
        .is_empty());
    drain(&c).await;
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queue_effect_lease_does_not_block_async_metadata_lookups() {
    let vault = Arc::new(MemoryVault::default());
    *vault.saved.lock().unwrap() = Some(credentials());
    let c = Controller::with_parts(path(), vault, owner, client).unwrap();
    let status = c.apply_enabled_from_config(true).await.unwrap();
    let id = status.connection_id.unwrap();
    let registry = c.registry();
    let connection = registry.selected().unwrap().unwrap();
    let (start, started) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let held = registry.clone();
    let worker = std::thread::spawn(move || {
        held.with_current(&connection, || {
            start.send(()).unwrap();
            wait.recv().unwrap();
            Ok(())
        })
    });
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    let began = Instant::now();
    let read = c.read(registry.albums(
        id,
        String::new(),
        super::super::catalog::Sort::Name,
        0,
        Some(18),
    ));
    assert!(
        matches!(tokio::time::timeout(Duration::from_millis(500),read).await.unwrap(),Err(e)if e==UNAVAILABLE)
    );
    assert!(began.elapsed() < Duration::from_millis(500));
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    drain(&c).await;
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_cleanup_waits_for_effect_lease_without_blocking_the_async_runtime() {
    let vault=Arc::new(MemoryVault::default());*vault.saved.lock().unwrap()=Some(credentials());
    let c=Controller::with_parts(path(),vault,owner,client).unwrap();c.apply_enabled_from_config(true).await.unwrap();
    let registry=c.registry();let connection=registry.selected().unwrap().unwrap();let retained=connection.clone();
    let (start,started)=std::sync::mpsc::channel();let (release,wait)=std::sync::mpsc::channel();
    let held=registry.clone();let effect=std::thread::spawn(move||held.with_current(&connection,||{
        start.send(()).unwrap();
        // Watchdog bounds the old implementation's runtime stall; no test can
        // deadlock the host while demonstrating the destructor problem.
        let _=wait.recv_timeout(Duration::from_secs(1));Ok(())
    }));
    started.recv_timeout(Duration::from_secs(2)).unwrap();c.begin_shutdown();
    let began=Instant::now();tokio::time::sleep(Duration::from_millis(50)).await;
    let responsive=began.elapsed()<Duration::from_millis(500);
    let _=release.send(());effect.join().unwrap().unwrap();drain(&c).await;
    assert!(!retained.active());assert_eq!(c.context.gate.state.lock().unwrap().1,0);assert!(!c.context.gate.state.lock().unwrap().2);
    assert!(responsive,"terminal cleanup blocked the async runtime behind the effect lease");
}

async fn source_fixture() -> (String,tokio::sync::oneshot::Sender<()>,tokio::task::JoinHandle<()>) {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let (stop,mut stopping)=tokio::sync::oneshot::channel();
    let worker=tokio::spawn(async move{loop {
        let mut socket=tokio::select!{_= &mut stopping=>break,result=listener.accept()=>result.unwrap().0};
        let mut bytes=Vec::new();while !bytes.ends_with(b"\r\n\r\n"){bytes.push(socket.read_u8().await.unwrap());assert!(bytes.len()<16384);}
        let request=String::from_utf8(bytes).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let url=reqwest::Url::parse(&format!("http://fixture.invalid{target}")).unwrap();
        let body=match url.path(){
            "/rest/ping.view"=>serde_json::json!({"subsonic-response":{"status":"ok"}}).to_string().into_bytes(),
            "/rest/getAlbum.view"=>serde_json::json!({"subsonic-response":{"status":"ok","album":{"id":"album","name":"Album","artist":"Artist","coverArt":"cover","song":[{"id":"one","title":"One","discNumber":1,"track":1,"duration":61.5},{"id":"two","title":"Two","discNumber":1,"track":2,"duration":122}]}}}).to_string().into_bytes(),
            "/rest/getSong.view"=>{let id=url.query_pairs().find(|(key,_)|key=="id").unwrap().1.into_owned();serde_json::json!({"subsonic-response":{"status":"ok","song":{"duration":if id=="one"{61.5}else{122.0},"id":id,"albumId":"album","artistId":"artist"}}}).to_string().into_bytes()},
            "/rest/stream.view"=>b"fLaC isolated container fixture".to_vec(),
            _=>panic!("unexpected fixture endpoint"),
        };
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n",body.len()).as_bytes()).await.unwrap();socket.write_all(&body).await.unwrap();
    }});(format!("http://{address}/"),stop,worker)
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn actual_nav_transport_prepares_duplicate_occurrences_and_streams_only_after_explicit_play_preparation(){
    use crate::windows_mpd::{library::Selection,transport::{self,MediaLease},queue_v3::Source};
    let (server,stop,peer)=source_fixture().await;let profile=path();
    let c=Controller::with_parts(profile.clone(),Arc::new(MemoryVault::default()),owner,client).unwrap();c.apply_enabled_from_config(true).await.unwrap();let status=c.connect(server,"fixture".into(),"PRIVATE-fixture".into()).await.unwrap();let id=status.connection_id.clone().unwrap();let account=status.account_id.clone().unwrap();
    let connection=c.registry().selected().unwrap().unwrap();assert!(!connection.playback_listener_started());
    let locator=crate::audio::MediaLocator::Navidrome{connection_id:id.clone(),account_id:Some(account.clone()),song_id:"one".into()};
    let availability=c.available_many(vec![(0,locator.clone()),(1,locator)]).await;assert_eq!(availability.len(),2);assert!(availability.iter().all(|(_,value)|matches!(value,Ok(true))),"{availability:?}");assert!(!connection.playback_listener_started());
    c.album(id.clone(),"album".into()).await.unwrap();assert!(!connection.playback_listener_started());
    let album=c.prepared_album(&id,"album",1).unwrap();assert!(connection.playback_listener_started());assert_eq!(album.tracks[1].title,"Two");
    let mut tracks=vec![album.tracks[0].clone(),album.tracks[0].clone(),album.tracks[1].clone()];
    let mut selected=Selection{root:Default::default(),tracks:tracks.clone(),start:1,album_id:None};
    let (mut leases,uris)=transport::admit_tracks(None,&mut selected,||Ok(())).unwrap();assert_eq!(leases.len(),3);assert_eq!(uris[0],uris[1]);assert_ne!(uris[1],uris[2]);assert!(leases.iter().all(|lease|lease.healthy()));
    let body=reqwest::Client::builder().no_proxy().build().unwrap().get(&uris[0]).send().await.unwrap().bytes().await.unwrap();assert_eq!(body.as_ref(),b"fLaC isolated container fixture");
    let plan=crate::windows_mpd::queue_edit::plan(3,Some(1),false,crate::windows_mpd::queue_edit::Edit::Move{from:0,to:2}).unwrap();crate::windows_mpd::queue_edit::apply(&mut leases,plan);crate::windows_mpd::queue_edit::apply(&mut tracks,plan);assert_eq!(tracks[1].title,"Two");
    let selected=c.prepare_tracks(tracks).await.unwrap();assert_eq!(selected.len(),3);assert_eq!(selected.iter().map(|t|t.remote_lease.as_ref().unwrap().presentation_duration()).collect::<Vec<_>>(),[Some(61.5),Some(122.0),Some(61.5)]);
    let logical=crate::windows_mpd::TestSavedQueue::capture(std::path::Path::new(""),&selected,0,14.0,100.0,73,None);assert!(logical.tracks().iter().all(|track|track.remote_lease.is_none()&&track.remote_connection_id.is_none()));
    let ready=c.prepare_tracks(logical.tracks()).await.unwrap();assert!(ready.iter().all(|track|track.remote_lease.is_some()));assert_eq!(ready.iter().map(|t|t.remote_lease.as_ref().unwrap().presentation_duration()).collect::<Vec<_>>(),[Some(61.5),Some(122.0),Some(61.5)]);assert!(ready.iter().all(|track|matches!(track.remote_source,Some(Source::Navidrome{..}))));
    let lease=match &leases[0]{MediaLease::Remote(lease)=>lease.clone(),_=>panic!("wrong transport")};
    let mut effects=0;assert_eq!(lease.command(&mut||{effects+=1;Ok(vec!["OK".into()])}).unwrap(),["OK"]);assert_eq!(effects,1);
    c.disconnect().await.unwrap();assert!(!lease.healthy());assert!(lease.command(&mut||{effects+=1;Ok(vec![])}).is_err());assert_eq!(effects,1);assert!(c.prepare_tracks(logical.tracks()).await.is_err());assert_eq!(logical.tracks().len(),3);
    let _=stop.send(());peer.await.unwrap();drain(&c).await;
    // The synthetic account vault is in memory; preparing never created files.
    assert!(!profile.parent().unwrap().exists());
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn real_mixed_local_nav_admission_preserves_local_handles_and_rejects_unprepared_or_wrong_sources(){
    use crate::windows_mpd::{library::Selection,transport::{self,MediaLease},queue_v3::Source};
    let (server,stop,peer)=source_fixture().await;let c=Controller::with_parts(path(),Arc::new(MemoryVault::default()),owner,client).unwrap();c.apply_enabled_from_config(true).await.unwrap();let status=c.connect(server,"fixture".into(),"PRIVATE-fixture".into()).await.unwrap();let id=status.connection_id.unwrap();c.album(id.clone(),"album".into()).await.unwrap();let album=c.prepared_album(&id,"album",0).unwrap();
    let root=std::env::temp_dir().join(format!("sonante-mixed-{}",api::random().unwrap()));std::fs::create_dir(&root).unwrap();let root=root.canonicalize().unwrap();let file=root.join("fixture.flac");std::fs::write(&file,b"fLaC isolated local container").unwrap();
    let mut local=album.tracks[0].clone();local.remote_source=None;local.remote_lease=None;local.remote_connection_id=None;local.path=file.clone();local.media_root=Some(root.clone());local.id="local".into();
    let mut selection=Selection{root:root.clone(),tracks:vec![local.clone(),album.tracks[0].clone(),local.clone(),album.tracks[0].clone()],start:2,album_id:None};
    let local_server=crate::windows_mpd::TestMediaServer::new().unwrap();let (mut leases,uris)=transport::admit_tracks(Some(&local_server),&mut selection,||Ok(())).unwrap();assert_eq!(leases.len(),4);assert_ne!(uris[0],uris[2]);assert_eq!(uris[1],uris[3]);assert_eq!(selection.tracks[2].media_root,Some(root.clone()));assert!(selection.tracks[1].path.as_os_str().is_empty());
    let removed=leases.remove(0);drop(removed);assert!(leases.iter().all(|lease|lease.healthy()));assert!(matches!(&leases[0],MediaLease::Remote(_)));assert!(matches!(&leases[1],MediaLease::Local(_)));
    let mut missing=Selection{root:Default::default(),tracks:vec![album.tracks[0].clone()],start:0,album_id:None};missing.tracks[0].remote_lease=None;assert!(matches!(transport::admit_tracks(None,&mut missing,||Ok(())),Err("NAVIDROME_CONTEXT_CHANGED")));
    let mut wrong=Selection{root:Default::default(),tracks:vec![album.tracks[0].clone()],start:0,album_id:None};wrong.tracks[0].remote_source=Some(Source::Navidrome{account_id:"b".repeat(64),song_id:"one".into()});assert!(transport::admit_tracks(None,&mut wrong,||Ok(())).is_err());
    assert!(transport::enabled(&selection.tracks,crate::windows_mpd::library::Sources{local:false,navidrome:true,plex:false}).is_err());assert!(transport::enabled(&selection.tracks,crate::windows_mpd::library::Sources{local:true,navidrome:false,plex:false}).is_err());transport::enabled(&selection.tracks,crate::windows_mpd::library::Sources{local:true,navidrome:true,plex:false}).unwrap();
    drop(leases);drop(local_server);std::fs::remove_dir_all(root).unwrap();let _=stop.send(());peer.await.unwrap();drain(&c).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn native_playlist_keeps_account_references_and_probes_without_starting_playback_or_deleting_offline_items(){
    let (server,stop,peer)=source_fixture().await;let c=Controller::with_parts(path(),Arc::new(MemoryVault::default()),owner,client).unwrap();c.apply_enabled_from_config(true).await.unwrap();let status=c.connect(server,"fixture".into(),"PRIVATE-fixture".into()).await.unwrap();let connection=c.registry().selected().unwrap().unwrap();
    let root=std::env::temp_dir().join(format!("sonante-nav-playlist-{}",api::random().unwrap()));std::fs::create_dir(&root).unwrap();
    let library=crate::windows_mpd::library::Library::new(root.join("folders.json"));library.configure(None,crate::windows_mpd::library::Sources{local:false,navidrome:true,plex:false}).unwrap();
    let store=crate::playlists::PlaylistStore::new(root.join("playlists.json"));
    let item=serde_json::json!({"media_locator":{"kind":"navidrome","account_id":status.account_id.unwrap(),"song_id":"one","connection_id":status.connection_id.unwrap()},"metadata":{"title":"One","artist":"Artist","album":"Album"}});
    let encoded=crate::windows_playlists::dispatch(&store,"create_playlist_with_items",&serde_json::json!({"name":"Remote","items":[item.clone(),item]})).unwrap();let playlist:crate::playlists::Playlist=serde_json::from_value(encoded).unwrap();assert_ne!(playlist.items[0].id,playlist.items[1].id);
    let bytes=std::fs::read_to_string(root.join("playlists.json")).unwrap();assert!(!bytes.contains("PRIVATE")&&!bytes.contains("http")&&!bytes.contains("127.0.0.1"));assert!(playlist.items.iter().all(|item|matches!(&item.media_locator,crate::audio::MediaLocator::Navidrome{connection_id,..} if connection_id.is_empty())));
    let a=c.clone();let lib=library.clone();let value=playlist.clone();let rows=tokio::task::spawn_blocking(move||crate::windows_playlists::resolve_sources(&value,&lib,&a)).await.unwrap().unwrap();assert!(rows.iter().all(|row|row.status=="available"),"{rows:?}");assert!(!connection.playback_listener_started());
    c.disconnect().await.unwrap();let a=c.clone();let lib=library.clone();let value=playlist.clone();let rows=tokio::task::spawn_blocking(move||crate::windows_playlists::resolve_sources(&value,&lib,&a)).await.unwrap().unwrap();assert!(rows.iter().all(|row|row.status=="unavailable"));assert_eq!(store.get(&playlist.id).unwrap().items.len(),2);assert_eq!(std::fs::read_to_string(root.join("playlists.json")).unwrap(),bytes);
    let _=stop.send(());peer.await.unwrap();drain(&c).await;library.stop();std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn editions_use_complete_catalog_and_current_account_without_media_listener(){
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
 let requests=Arc::new(Mutex::new(Vec::new()));let seen=requests.clone();let(stop,mut stopped)=tokio::sync::watch::channel(false);
 let pending=Arc::new(tokio::sync::Notify::new());let admitted=pending.clone();let released=Arc::new(tokio::sync::Notify::new());let release=released.clone();let block=Arc::new(std::sync::atomic::AtomicBool::new(false));let blocked=block.clone();
 let peer=tokio::spawn(async move{loop{let accepted=tokio::select!{_=stopped.changed()=>break,r=listener.accept()=>r.unwrap()};let(mut socket,_)=accepted;
  let mut request=Vec::new();while !request.ends_with(b"\r\n\r\n"){request.push(socket.read_u8().await.unwrap());assert!(request.len()<8192);}let request=String::from_utf8(request).unwrap();assert!(!request.contains("PRIVATE-fixture"));
  let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let url=reqwest::Url::parse(&format!("http://{addr}{target}")).unwrap();seen.lock().unwrap().push(url.path().to_owned());
  let body=match url.path(){
   "/rest/getAlbumList2.view"=>serde_json::json!({"subsonic-response":{"status":"ok","albumList2":{"album":[{"id":"one","name":"Own Album","artist":"Artist","songCount":2,"coverArt":"own-cover","year":1998}]}}}),
   "/rest/getAlbum.view"=>{assert!(url.query_pairs().any(|(k,v)|k=="id"&&v=="one"));if blocked.load(Ordering::Acquire){admitted.notify_one();tokio::time::timeout(Duration::from_secs(3),release.notified()).await.unwrap();}serde_json::json!({"subsonic-response":{"status":"ok","album":{"id":"one","songCount":2,"coverArt":"own-cover","song":[{"id":"t1","albumId":"one","title":"One","size":123},{"id":"t2","albumId":"one","title":"Two","size":456}]}}})},
   other=>panic!("Unexpected fixture path {other}"),
  }.to_string();let _=socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Total-Count: 1\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
 }});
 let vault=Arc::new(MemoryVault::default());let mut cred=credentials();cred.server=format!("http://{addr}/");*vault.saved.lock().unwrap()=Some(cred);
 let host=Controller::with_parts(path(),vault.clone(),owner,client).unwrap();let status=host.apply_enabled_from_config(true).await.unwrap();let id=status.connection_id.unwrap();let account=status.account_id.unwrap();let connection=host.registry().current(&id).unwrap();
 let albums=host.edition_catalog(id.clone(),account.clone()).await.unwrap();assert_eq!(albums.len(),1);let value=serde_json::to_value(&albums[0]).unwrap();assert_eq!(value["cover_id"],"own-cover");assert_eq!(value["song_count"],2);assert_eq!(value["year"],1998);
 assert_eq!(host.smart_catalog(account.clone()).await.unwrap().len(),1);assert_eq!(requests.lock().unwrap().len(),1,"same complete cache, no second scan or tracks");
 let before=requests.lock().unwrap().len();assert!(host.edition_catalog("old".into(),account.clone()).await.is_err());assert!(host.edition_profile(id.clone(),"wrong-account".into(),"one".into()).await.is_err());assert_eq!(requests.lock().unwrap().len(),before);
 let p=host.edition_profile(id.clone(),account.clone(),"one".into()).await.unwrap().unwrap();assert_eq!(p.sizes,Some(vec![123,456]));assert_eq!(p.titles,Some(vec!["One".into(),"Two".into()]));assert!(!connection.playback_listener_started());assert_eq!(vault.writes.load(Ordering::SeqCst),0);
 block.store(true,Ordering::Release);let h=host.clone();let reading=tokio::spawn(async move{h.edition_profile(id,account,"one".into()).await});tokio::time::timeout(Duration::from_secs(3),pending.notified()).await.unwrap();host.apply_enabled_from_config(false).await.unwrap();released.notify_one();assert!(reading.await.unwrap().is_err());assert!(!connection.active());assert!(!connection.playback_listener_started());drain(&host).await;stop.send_replace(true);peer.await.unwrap();
}
