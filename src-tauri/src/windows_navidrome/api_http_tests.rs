use super::*;
use std::io::{Read, Write};
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_http_auth_prefix_count_errors_and_body_caps_use_the_original_client_contract() {
    for mode in [
        "ok", "auth", "status", "redirect", "envelope", "length", "chunked",
    ] {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let end = std::time::Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(std::time::Instant::now() < end);
                std::thread::sleep(Duration::from_millis(2));
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") && header.len() < 8192 {
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            let target = header
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = Url::parse(&format!("http://{addr}{target}")).unwrap();
            assert_eq!(url.path(), "/prefix/rest/ping.view");
            let params: std::collections::HashMap<_, _> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            assert_eq!(params["u"], "fixture & user");
            assert_eq!(params["s"].len(), 48);
            assert_eq!(
                params["t"],
                format!(
                    "{:x}",
                    md5::compute(format!("fixture-password-NOT-LOGGED{}", params["s"]))
                )
            );
            assert!(!params.contains_key("p"));
            assert!(!target.contains("fixture-password"));
            let good = r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#;
            let response=match mode{
    "ok"=>format!("HTTP/1.1 200 OK\r\nX-Total-Count: 123\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{good}",good.len()),
    "auth"=>"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE".into(),
    "status"=>"HTTP/1.1 503 Unavailable\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE".into(),
    "redirect"=>format!("HTTP/1.1 302 Found\r\nLocation: http://{addr}/forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    "length"=>format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",2*1024*1024+1),
    "chunked"=>format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",2*1024*1024+1,"x".repeat(2*1024*1024+1)),
    _=>"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".into(),
   };
            let _ = stream.write_all(response.as_bytes());
        });
        // Keep the fixture strictly on loopback even if a developer proxy is set.
        // Production client() remains copied unchanged and is compared separately.
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let credentials = Credentials {
            connection_id: "fixture".into(),
            server: format!("http://{addr}/prefix/"),
            username: "fixture & user".into(),
            password: "fixture-password-NOT-LOGGED".into(),
        };
        let result = request_counted(&client, &credentials, "ping", &[]).await;
        peer.join().unwrap();
        match mode {
            "ok" => {
                let (root, total) = result.unwrap();
                assert_eq!(root["status"], "ok");
                assert_eq!(total, Some(123));
            }
            "auth" => assert_eq!(result.unwrap_err(), AUTH),
            "status" | "redirect" => assert_eq!(result.unwrap_err(), UNAVAILABLE),
            _ => assert_eq!(result.unwrap_err(), PROTOCOL),
        }
    }
}
