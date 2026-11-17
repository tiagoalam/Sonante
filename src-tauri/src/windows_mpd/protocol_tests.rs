use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
#[test]
fn internal_volume_list_is_one_bounded_exchange_and_returns_only_confirmed_status(){
 let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
 let server=thread::spawn(move||{let (s,_)=listener.accept().unwrap();let mut r=BufReader::new(s);r.get_mut().write_all(b"OK MPD 0.24.18\n").unwrap();let mut line=String::new();r.read_line(&mut line).unwrap();assert_eq!(line,"password \"fixture\"\n");r.get_mut().write_all(b"OK\n").unwrap();
  for expected in ["command_list_begin\n","pause 1\n","setvol 100\n","status\n","command_list_end\n"]{line.clear();r.read_line(&mut line).unwrap();assert_eq!(line,expected);}
  r.get_mut().write_all(b"state: pause\nvolume: 100\nsongid: 7\nplaylist: 2\nOK\n").unwrap();
 });
 let mut client=Client::connect(address,"fixture",|_|Ok(())).unwrap();let lines=client.command_list(&["pause 1","setvol 100","status"]).unwrap();assert_eq!(field(&lines,"state").unwrap(),Some("pause"));assert_eq!(field(&lines,"volume").unwrap(),Some("100"));server.join().unwrap();
}
#[test]
fn internal_list_rejects_nested_lists_or_control_injection_without_writing_them(){
 let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
 let server=thread::spawn(move||{let (s,_)=listener.accept().unwrap();let mut r=BufReader::new(s);r.get_mut().write_all(b"OK MPD 0.24.18\n").unwrap();let mut line=String::new();r.read_line(&mut line).unwrap();r.get_mut().write_all(b"OK\n").unwrap();line.clear();r.read_line(&mut line).unwrap();assert_eq!(line,"status\n");r.get_mut().write_all(b"state: stop\nvolume: 0\nOK\n").unwrap();});
 let mut client=Client::connect(address,"fixture",|_|Ok(())).unwrap();
 for commands in [vec![],vec!["status\nplay"],vec!["command_list_begin"],vec!["status\0"]]{assert_eq!(client.command_list(&commands),Err("invalid_argument"));}
 assert_eq!(field(&client.command("status").unwrap(),"state").unwrap(),Some("stop"));server.join().unwrap();
}

#[test]
fn arguments_preserve_linux_escaping_contract_and_reject_injection() {
    assert_eq!(
        quote("Música \\\" ' álbum.flac").unwrap(),
        "\"Música \\\\\\\" ' álbum.flac\""
    );
    for value in ["x\nclear", "x\rplay", "x\0"] {
        assert_eq!(quote(value), Err("invalid_argument"));
    }
    assert_eq!(
        windows_argument("C:\\Music \\").unwrap(),
        "\"C:\\Music \\\\\""
    );
    assert_eq!(windows_argument("a\"b").unwrap(), "\"a\\\"b\"");
}

#[test]
fn incomplete_or_contradictory_status_is_never_available() {
    assert!(playback(&strings(&["state: play"])).is_err());
    assert!(playback(&strings(&["state: play", "state: stop", "volume: 50"])).is_err());
    for value in ["NaN", "inf", "-1"] {
        assert!(playback(&strings(&[
            "state: play",
            "volume: 50",
            &format!("elapsed: {value}")
        ]))
        .is_err());
    }
    assert_eq!(
        playback(&strings(&[
            "state: play",
            "volume: 50",
            "error: output failed"
        ])),
        Err("mpd_playback_error")
    );
    let value = playback(&strings(&[
        "state: pause",
        "volume: 47",
        "elapsed: 3.2",
        "duration: 60",
        "audio: 44100:16:2",
    ]))
    .unwrap();
    assert_eq!(value.state, "pause");
    assert_eq!(value.volume, 47);
    assert_eq!(value.elapsed, 3.2);
}

#[test]
fn unverified_peer_never_receives_a_password() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut data = [0; 128];
        assert_eq!(stream.read(&mut data).unwrap(), 0);
    });
    assert!(matches!(
        Client::connect(address, "secret", |_| Err("peer_unverified")),
        Err("peer_unverified")
    ));
    server.join().unwrap();
}

fn exchange(reply: &[u8]) -> Result<Vec<String>> {
    let reply = reply.to_vec();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(b"OK MPD 0.24.0\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "password \"secret\"\n");
        reader.get_mut().write_all(b"OK\n").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "status\n");
        reader.get_mut().write_all(&reply).unwrap();
    });
    let mut client = Client::connect(address, "secret", |_| Ok(())).unwrap();
    let result = client.command("status");
    server.join().unwrap();
    result
}

#[test]
fn ack_eof_partial_utf8_and_limits_are_not_success() {
    assert_eq!(
        exchange(b"ACK [52@0] {status} private path\n"),
        Err("mpd_ack")
    );
    assert_eq!(exchange(b"state: play\n"), Err("mpd_eof"));
    assert_eq!(exchange(b"state: play"), Err("mpd_eof"));
    assert_eq!(exchange(b"\xff\nOK\n"), Err("mpd_protocol"));
    assert_eq!(exchange(&vec![b'x'; LINE_LIMIT + 1]), Err("mpd_protocol"));
    assert_eq!(exchange(&b"x: y\n".repeat(1026)), Err("mpd_protocol"));
    assert_eq!(
        exchange(b"state: stop\nvolume: 50\nOK\n").unwrap(),
        strings(&["state: stop", "volume: 50"])
    );
}

#[test]
fn absent_or_malformed_greeting_does_not_authenticate() {
    for greeting in [b"OK MPD 0.24.0".as_slice(), b"other service\n".as_slice()] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let greeting = greeting.to_vec();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&greeting).unwrap();
        });
        assert!(Client::connect(address, "secret", |_| Ok(())).is_err());
        server.join().unwrap();
    }
}

#[test]
fn missing_final_response_times_out_instead_of_succeeding() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(b"OK MPD 0.24.0\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        reader.get_mut().write_all(b"OK\n").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        // A real half-open peer; no sleeps or audio processes are involved.
        let _ = reader.read_line(&mut String::new());
    });
    let start = Instant::now();
    {
        let mut client = Client::connect(address, "secret", |_| Ok(())).unwrap();
        assert_eq!(client.command("status"), Err("mpd_timeout"));
        assert_eq!(client.command("status"), Err("mpd_disconnected"));
    }
    assert!(start.elapsed() < Duration::from_secs(4));
    server.join().unwrap();
}
