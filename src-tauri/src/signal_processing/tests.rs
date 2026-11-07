use super::*;
use std::{fs, os::unix::net::UnixListener, path::PathBuf, sync::atomic::AtomicU64, thread};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}
fn fixture() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "sonante-output-metadata-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn mpd_volume_metadata_is_allowlisted_and_playback_status_is_unchanged() {
    let empty = ProcessingStatus::from_status(&[]);
    assert_eq!(empty.mpd_volume, None);
    for text in [
        "volume: -1",
        "volume: 101",
        "volume: 100\nvolume: 100",
        "volume: secret",
    ] {
        assert_eq!(ProcessingStatus::from_status(&lines(text)).mpd_volume, None);
    }
    let source = lines("state: play\naudio: 96000:24:2\nvolume: 43\nxfade: 2\nmixrampdelay: 0.5");
    let before = source.clone();
    let status = ProcessingStatus::from_status(&source);
    assert_eq!(status.mpd_volume, Some(43));
    assert_eq!(source, before);
}
#[test]
fn foreign_socket_peer_is_rejected_before_sending_any_command() {
    let dir = fixture();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut data = Vec::new();
        stream.read_to_end(&mut data).unwrap();
        data
    });
    assert_eq!(query_output_lines(&path, std::process::id() + 1), Err(ReadFailure::Changed));
    assert!(server.join().unwrap().is_empty());
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unresponsive_peer_is_bounded_and_missing_socket_is_unavailable() {
    let dir = fixture();
    let path = dir.join("s");
    assert_eq!(
        query_output_lines(&path, std::process::id()),
        Err(ReadFailure::Unavailable)
    );
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(220));
    });
    let started = Instant::now();
    assert_eq!(query_output_lines(&path, std::process::id()), Err(ReadFailure::Timeout));
    assert!(started.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn one_physical_read_and_automatic_permit_cleanup_have_no_wait_queue() {
    let flag = AtomicBool::new(false);
    let permit = enter(&flag).unwrap();
    assert!(enter(&flag).is_none());
    drop(permit);
    assert!(enter(&flag).is_some());
}

#[test]
fn trickled_unterminated_line_obeys_the_whole_query_deadline() {
    let dir = fixture();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        for _ in 0..15 {
            if stream.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
    });
    let started = Instant::now();
    assert_eq!(query_output_lines(&path, std::process::id()), Err(ReadFailure::Timeout));
    assert!(started.elapsed() < Duration::from_millis(450));
    server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
}

fn transport_lines(value: &str) -> Vec<String> {
    lines(&format!("outputid: 0\noutputname: Sonante Output\nplugin: alsa\noutputenabled: 1\nattribute: sonante_transport={value}\n"))
}
#[test]
fn negotiated_transport_is_bounded_and_only_from_one_enabled_main_alsa_output() {
    for (kind, expected) in [
        ("native_dsd", TransportKind::NativeDsd),
        ("dop", TransportKind::Dop),
        ("pcm", TransportKind::Pcm),
    ] {
        let parsed =
            parse_transport(&transport_lines(&format!("{kind}:352800:176400:2:1"))).unwrap();
        assert_eq!(parsed.kind, expected);
        assert_eq!(parsed.source_rate, 352800);
        assert_eq!(parsed.output_rate, 176400);
        assert_eq!(parsed.epoch, 1);
    }
    for value in [
        "closed",
        "dop:1:176400:2:1",
        "dop:352800:0:2:1",
        "dop:352800:176400:0:1",
        "dop:352800:176400:65:1",
        "dop:352800:176400:2:0",
        "dop:352800:176400:2:-1",
        "dop:352800:176400:2:18446744073709551616",
        "fake:352800:176400:2:1",
        "dop:352800:176400:2:1:private",
    ] {
        assert_eq!(parse_transport(&transport_lines(value)), None, "{value}");
    }
    let good = transport_lines("dop:352800:176400:2:1");
    for (before, after) in [
        ("Sonante Output", "Sonante Analyzer"),
        ("plugin: alsa", "plugin: null"),
        ("outputenabled: 1", "outputenabled: 0"),
    ] {
        assert_eq!(
            parse_transport(
                &good
                    .iter()
                    .map(|s| s.replace(before, after))
                    .collect::<Vec<_>>()
            ),
            None
        );
    }
    let mut duplicate = good.clone();
    duplicate.push("attribute: sonante_transport=dop:352800:176400:2:1".into());
    assert_eq!(parse_transport(&duplicate), None);
    let mut multiple = good.clone();
    multiple.extend(good);
    assert_eq!(parse_transport(&multiple), None);
}
#[test]
fn negotiated_transport_socket_only_reads_outputs_and_does_not_mutate_playback() {
    let dir = fixture();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"OK MPD 0.24.15\n").unwrap();
        let mut call = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut call)
            .unwrap();
        let reply = transport_lines("dop:352800:176400:2:1").join("\n") + "\nOK\n";
        stream.write_all(reply.as_bytes()).unwrap();
        call
    });
    let result = query_output_lines(&path, std::process::id()).unwrap();
    assert_eq!(parse_transport(&result).unwrap().kind, TransportKind::Dop);
    assert_eq!(server.join().unwrap(), "outputs\n");
    fs::remove_dir_all(dir).unwrap();
}
