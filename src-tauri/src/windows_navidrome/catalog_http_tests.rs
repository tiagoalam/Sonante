use super::super::proxy::windows_contract_tests::credentials;
use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn reply(url: &reqwest::Url) -> Vec<u8> {
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    let id = params.get("id").map(|s| s.as_ref()).unwrap_or("");
    let album = serde_json::json!({"id":"album","name":"Fixture Album","artist":"Fixture Artist","artistId":"artist","coverArt":"art","songCount":2,"song":[{"id":"song-b","title":"Second","track":1,"discNumber":2},{"id":"song-a","title":"First","track":4,"discNumber":1,"duration":120}]});
    let root = match url.path() {
        "/prefix/rest/getAlbumList2.view" => {
            serde_json::json!({"albumList2":{"album":[album.clone()]}})
        }
        "/prefix/rest/getAlbum.view" => {
            serde_json::json!({"album":if id=="wrong"{serde_json::json!({"id":"other","name":"Wrong","song":[]})}else{album.clone()}})
        }
        "/prefix/rest/getArtist.view" => {
            serde_json::json!({"artist":{"id":"artist","name":"Fixture Artist","album":[album.clone()]}})
        }
        "/prefix/rest/search3.view" => {
            serde_json::json!({"searchResult3":{"artist":[{"id":"artist","name":"Fixture Artist"}],"album":[album.clone()],"song":[{"id":"song-a","title":"First","artist":"Fixture Artist","album":"Fixture Album","albumId":"album","duration":120,"streamUrl":"http://PRIVATE.invalid/?token=PRIVATE"}]}})
        }
        "/prefix/rest/getSong.view" => {
            serde_json::json!({"song":{"id":id,"albumId":"album","artistId":"artist","coverArt":"art"}})
        }
        "/prefix/rest/getCoverArt.view" => {
            let bytes = match id {
                "invalid" => b"<svg>PRIVATE</svg>".to_vec(),
                "oversize" => vec![0; 512 * 1024 + 1],
                _ => b"\x89PNG\r\n\x1a\nfixture".to_vec(),
            };
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .into_bytes();
            out.extend(bytes);
            return out;
        }
        _ => panic!("unexpected fixture endpoint"),
    };
    let mut envelope = serde_json::json!({"subsonic-response":{"status":"ok"}});
    for (key, value) in root.as_object().unwrap() {
        envelope["subsonic-response"][key] = value.clone();
    }
    let bytes = serde_json::to_vec(&envelope).unwrap();
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Total-Count: 123\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .into_bytes();
    out.extend(bytes);
    out
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_catalog_discovery_and_artwork_run_through_one_account_bound_http_fixture() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let (stop, mut receiver) = tokio::sync::watch::channel(false);
    let peer = tokio::spawn(async move {
        let mut children = tokio::task::JoinSet::new();
        loop {
            let accepted =
                tokio::select! {_ =receiver.changed()=>break,result=listener.accept()=>result};
            let (mut socket, _) = accepted.unwrap();
            let observed = observed.clone();
            children.spawn(async move {
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    bytes.push(socket.read_u8().await.unwrap());
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
                observed.lock().unwrap().push(url.path().to_string());
                let _ = socket.write_all(&reply(&url)).await;
            });
        }
        while let Some(child) = children.join_next().await {
            child.unwrap();
        }
    });
    let c = Connection::fixture(credentials(
        format!("http://{address}/prefix/"),
        "SESSION",
        "fixture-user",
    ))
    .await;
    let registry = Registry::default();
    registry.replace(Some(c.clone())).unwrap();
    let page = registry
        .albums("SESSION".into(), String::new(), Sort::Name, 0, Some(18))
        .await
        .unwrap();
    let page = serde_json::to_value(page).unwrap();
    assert_eq!(page["total"], 123);
    assert_eq!(page["more"], true);
    assert_eq!(page["albums"][0]["id"], "album");
    let detail = registry
        .album("SESSION".into(), "album".into())
        .await
        .unwrap();
    assert_eq!(detail.songs[0].id, "song-a");
    assert_eq!(detail.songs[1].id, "song-b");
    assert_eq!(
        registry.prepared("SESSION", "album").unwrap().songs.len(),
        2
    );
    assert_eq!(
        registry
            .song_cover_id("SESSION".into(), "song-a".into())
            .await
            .unwrap()
            .as_deref(),
        Some("art")
    );
    assert!(
        !requests
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.ends_with("getSong.view")),
        "prepared artwork uses the existing bounded detail cache"
    );
    assert_eq!(
        registry
            .song_cover_id("SESSION".into(), "uncached-song".into())
            .await
            .unwrap()
            .as_deref(),
        Some("art")
    );
    let cover = registry
        .cover("SESSION".into(), "art".into())
        .await
        .unwrap();
    assert!(cover.starts_with("data:image/png;base64,"));
    for invalid in ["invalid", "oversize"] {
        assert!(
            matches!(registry.cover("SESSION".into(),invalid.into()).await,Err(e)if e==PROTOCOL)
        );
    }
    assert!(matches!(registry.album("SESSION".into(),"wrong".into()).await,Err(e)if e==PROTOCOL));
    let results = serde_json::to_value(
        registry
            .search("SESSION".into(), "fixture".into())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(results["songs"][0]["album_id"], "album");
    assert!(!results.to_string().contains("PRIVATE"));
    let artist = serde_json::to_value(
        registry
            .artist_detail("SESSION".into(), "artist".into())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(artist["albums"][0]["id"], "album");
    let explore = serde_json::to_value(registry.explore("SESSION".into()).await.unwrap()).unwrap();
    assert_eq!(explore["suggested"].as_array().unwrap().len(), 1);
    assert_eq!(explore["added"].as_array().unwrap().len(), 1);
    let navigation = serde_json::to_value(
        registry
            .player_navigation("SESSION".into(), "song-a".into())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(navigation["album_id"], "album");
    assert_eq!(navigation["account_id"], c.account());
    let before = requests.lock().unwrap().len();
    assert!(registry
        .albums("SESSION".into(), String::new(), Sort::Name, 0, Some(19))
        .await
        .is_err());
    assert!(registry.search("SESSION".into(), "x".into()).await.is_err());
    assert_eq!(requests.lock().unwrap().len(), before);
    registry.replace(None).unwrap();
    assert!(registry.prepared("SESSION", "album").is_err());
    stop.send_replace(true);
    peer.await.unwrap();
}
#[tokio::test]
async fn original_prepared_cache_limits_and_retirement_while_waiting_for_artwork_are_preserved() {
    let c = Connection::fixture(credentials(
        "http://127.0.0.1:9/".into(),
        "SESSION",
        "fixture",
    ))
    .await;
    for i in 0..10 {
        let mut detail = Detail {
            album: api::album(&serde_json::json!({"id":format!("album-{i}"),"name":"Fixture"}))
                .unwrap(),
            songs: vec![],
            artist_id: None,
        };
        c.remember(&detail).unwrap();
        if i == 9 {
            detail.album.id = "too-many".into();
            detail.songs = (0..501)
                .map(|i| Song {
                    id: format!("song-{i}"),
                    title: "Fixture".into(),
                    artist: "Fixture".into(),
                    album: "Fixture".into(),
                    duration: None,
                    disc: 1,
                    track: i,
                })
                .collect();
            c.remember(&detail).unwrap();
            assert!(c.prepared("too-many").is_err());
        }
    }
    assert!(c.prepared("album-0").is_err());
    assert!(c.prepared("album-1").is_err());
    assert!(c.prepared("album-2").is_ok());
    assert_eq!(c.details.lock().unwrap().len(), 8);
    let registry = Arc::new(Registry::default());
    registry.replace(Some(c.clone())).unwrap();
    let permit = c.covers.acquire_many(3).await.unwrap();
    let mut pending = std::pin::pin!(registry.song_cover_id("SESSION".into(), "unknown".into()));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(pending.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    c.retire();
    assert!(
        matches!(tokio::time::timeout(std::time::Duration::from_secs(1),pending).await.unwrap(),Err(e)if e==super::super::CHANGED)
    );
    drop(permit);
}

#[tokio::test]
async fn opening_saved_credentials_uses_a_fresh_generation_without_requesting_the_server() {
    let before = credentials("http://127.0.0.1:9/".into(), "OLD-SAVED-SESSION", "fixture");
    let one = Connection::open(before.clone()).await.unwrap();
    let two = Connection::open(before).await.unwrap();
    assert_ne!(one.credentials.connection_id, "OLD-SAVED-SESSION");
    assert_ne!(one.credentials.connection_id, two.credentials.connection_id);
    assert_eq!(one.credentials.connection_id.len(), 48);
    assert_eq!(one.account(), two.account());
    one.retire();
    two.retire();
}
