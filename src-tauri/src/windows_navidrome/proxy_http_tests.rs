use super::super::{api::Credentials, connection::Connection};
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn upstream(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let job = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let byte = tokio::time::timeout(Duration::from_secs(2), socket.read_u8())
                .await
                .unwrap()
                .unwrap();
            bytes.push(byte);
            assert!(bytes.len() < 8192);
        }
        socket.write_all(&response).await.unwrap();
        String::from_utf8(bytes).unwrap()
    });
    (format!("http://{addr}/prefix/"), job)
}
pub(crate) fn credentials(server: String, session: &str, user: &str) -> Credentials {
    Credentials {
        connection_id: session.into(),
        server,
        username: user.into(),
        password: "fixture-PRIVATE-password".into(),
    }
}
async fn send(uri: &str, method: &str, headers: &str) -> Vec<u8> {
    let url = reqwest::Url::parse(uri).unwrap();
    let mut socket = TcpStream::connect(("127.0.0.1", url.port().unwrap()))
        .await
        .unwrap();
    socket
        .write_all(
            format!(
                "{method} {} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n",
                url.path()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut body = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut body))
        .await
        .unwrap();
    // Saturation closes an accepted socket without reading it. Platforms may
    // report either EOF or reset; neither is an HTTP success response.
    if let Err(error) = result {
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    }
    body
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_loopback_bridge_preserves_range_head_error_policy_and_retires_pending_io() {
    for method in ["GET", "HEAD"] {
        let (server, upstream) = upstream(b"HTTP/1.1 206 Partial Content\r\nContent-Type: audio/flac\r\nContent-Range: bytes 1-4/8\r\nAccept-Ranges: bytes\r\nContent-Length: 4\r\nX-Private: PRIVATE\r\nConnection: close\r\n\r\nDATA".to_vec()).await;
        let c = Connection::fixture(credentials(server, "session", "fixture-user")).await;
        let uri = c.stream("song-1").unwrap();
        let body = send(&uri, method, "Range: bytes=1-4\r\n").await;
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(text.contains("content-range: bytes 1-4/8\r\n"));
        assert!(text.contains("Content-Length: 4\r\n"));
        assert!(!text.contains("PRIVATE"));
        assert_eq!(text.ends_with("DATA"), method == "GET");
        let request = upstream.await.unwrap();
        assert!(request.starts_with("GET /prefix/rest/stream.view?"));
        assert!(request.to_lowercase().contains("range: bytes=1-4\r\n"));
        let target = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let parsed = reqwest::Url::parse(&format!("http://fixture.invalid{target}")).unwrap();
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(params.get("id").unwrap(), "song-1");
        assert_eq!(params.get("format").unwrap(), "raw");
        assert_eq!(params.get("maxBitRate").unwrap(), "0");
        assert!(!target.contains("PRIVATE"));
        c.retire();
        assert!(c.stream("song-1").is_err());
    }
    for (status, content, expected) in [
        (401, "audio/flac", "502"),
        (200, "application/json", "502"),
        (416, "audio/flac", "416"),
    ] {
        let (server, upstream) = upstream(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()).await;
        let c = Connection::fixture(credentials(server, "session", "fixture-user")).await;
        assert!(
            String::from_utf8(send(&c.stream("song").unwrap(), "GET", "").await)
                .unwrap()
                .starts_with(&format!("HTTP/1.1 {expected}"))
        );
        upstream.await.unwrap();
        c.retire();
    }
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}/", upstream.local_addr().unwrap());
    let c = Connection::fixture(credentials(server, "session", "fixture-user")).await;
    let uri = c.stream("song").unwrap();
    for (method, headers) in [
        ("POST", ""),
        ("GET", "Range: bytes=1-2\r\nRange: bytes=3-4\r\n"),
    ] {
        assert!(String::from_utf8(send(&uri, method, headers).await)
            .unwrap()
            .starts_with("HTTP/1.1 404"));
    }
    assert!(
        String::from_utf8(send(&uri.replace(&c.secret, "wrong"), "GET", "").await)
            .unwrap()
            .starts_with("HTTP/1.1 404")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), upstream.accept())
            .await
            .is_err(),
        "invalid capability must not request upstream"
    );
    // Block after upstream admission, then retire. The pending socket operation
    // must end promptly without waiting for the upstream response/read timeout.
    let client = tokio::spawn(async move { send(&uri, "GET", "").await });
    let (_socket, _) = tokio::time::timeout(Duration::from_secs(2), upstream.accept())
        .await
        .unwrap()
        .unwrap();
    let mut late = c.stop.subscribe();
    c.retire();
    assert!(
        *late.wait_for(|value| *value).await.unwrap(),
        "late subscriber observes retirement without lost wake"
    );
    assert!(tokio::time::timeout(Duration::from_secs(1), client)
        .await
        .unwrap()
        .unwrap()
        .is_empty());
    // Saturate six different live connections, not six tasks of just one
    // connection. A seventh connection must not get another upstream stream.
    for _ in 0..100 {
        if slots().available_permits() == 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(slots().available_permits(), 6);
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}/", upstream.local_addr().unwrap());
    let mut connections = Vec::new();
    let mut clients = Vec::new();
    let mut upstream_sockets = Vec::new();
    for i in 0..6 {
        let c = Connection::fixture(credentials(
            server.clone(),
            &format!("session-{i}"),
            "fixture-user",
        ))
        .await;
        let uri = c.stream("song").unwrap();
        clients.push(tokio::spawn(async move { send(&uri, "GET", "").await }));
        upstream_sockets.push(
            tokio::time::timeout(Duration::from_secs(2), upstream.accept())
                .await
                .unwrap()
                .unwrap()
                .0,
        );
        connections.push(c);
    }
    assert_eq!(slots().available_permits(), 0);
    let seventh = Connection::fixture(credentials(server, "seventh", "fixture-user")).await;
    let rejected = send(&seventh.stream("song").unwrap(), "GET", "").await;
    assert!(rejected.is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), upstream.accept())
            .await
            .is_err()
    );
    for c in &connections {
        c.retire();
    }
    seventh.retire();
    for client in clients {
        assert!(tokio::time::timeout(Duration::from_secs(1), client)
            .await
            .unwrap()
            .unwrap()
            .is_empty());
    }
    for _ in 0..100 {
        if slots().available_permits() == 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(slots().available_permits(), 6);
}
