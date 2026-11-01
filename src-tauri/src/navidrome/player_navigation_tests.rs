#[tokio::test]
async fn player_navigation_uses_exact_song_and_rejects_incomplete_remote_identity() {
    use std::io::{Read,Write};
    let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
    let peer=std::thread::spawn(move||{
        for _ in 0..4 {
            let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut request=Vec::new();while !request.ends_with(b"\r\n\r\n") {let mut byte=[0];assert_eq!(stream.read(&mut byte).unwrap(),1);request.push(byte[0]);}
            let request=String::from_utf8(request).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
            let url=reqwest::Url::parse(&format!("http://fixture{target}")).unwrap();assert!(url.path().ends_with("/getSong.view"));
            let id=url.query_pairs().find(|(key,_)|key=="id").unwrap().1.into_owned();
            let song=match id.as_str(){
                "valid"=>serde_json::json!({"id":"valid","albumId":"album","artistId":"artist"}),
                "missing-artist"=>serde_json::json!({"id":"missing-artist","albumId":"album"}),
                "wrong-song"=>serde_json::json!({"id":"another","albumId":"album","artistId":"artist"}),
                "wrong-album"=>serde_json::json!({"id":"wrong-album","albumId":"../unsafe","artistId":"artist"}),
                _=>panic!("unexpected song"),
            };
            let body=serde_json::json!({"subsonic-response":{"status":"ok","song":song}}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    let c=Connection::open(credentials("navigate",format!("http://{address}/"))).unwrap();
    let result=player_navigation(&c,"valid").await.unwrap();assert_eq!(result.album_id,"album");assert_eq!(result.artist_id.as_deref(),Some("artist"));
    assert_eq!(result.account_id,favorites::scope(&c.credentials));assert_eq!(result.connection_id,"navigate");
    let serialized=serde_json::to_string(&result).unwrap();for secret in [&c.secret,&c.credentials.password,&c.credentials.server] {assert!(!serialized.contains(secret));}
    assert!(player_navigation(&c,"missing-artist").await.unwrap().artist_id.is_none());
    for id in ["wrong-song","wrong-album"] {assert_eq!(player_navigation(&c,id).await.err().as_deref(),Some(PROTOCOL));}
    assert_eq!(player_navigation(&c,"../invalid").await.err().as_deref(),Some(INVALID));
    c.retire();assert_eq!(player_navigation(&c,"valid").await.err().as_deref(),Some(CHANGED));peer.join().unwrap();
}
