use super::*;
use std::{fs, os::unix::net::UnixListener, path::PathBuf, sync::atomic::AtomicU64, thread};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}
fn fixture() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "sonante-rg-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn incomplete_duplicate_or_unknown_processing_values_are_never_assumed_off_or_unity() {
    let empty = ProcessingStatus::from_status(&[]);
    assert_eq!(empty.mpd_volume, None);
    assert_eq!(empty.crossfade_seconds, None);
    assert_eq!(empty.mixramp, MixRamp::Unknown);
    assert_eq!(empty.replay_gain, ReplayGain::Unknown);
    for text in [
        "volume: -1",
        "volume: 101",
        "volume: 100\nvolume: 100",
        "volume: secret",
    ] {
        assert_eq!(ProcessingStatus::from_status(&lines(text)).mpd_volume, None);
    }
    for text in [
        "xfade: NaN",
        "xfade: inf",
        "xfade: -1",
        "xfade: 0\nxfade: 0",
    ] {
        assert_eq!(
            ProcessingStatus::from_status(&lines(text)).crossfade_seconds,
            None
        );
    }
    assert_eq!(
        ProcessingStatus::from_status(&lines("mixrampdelay: nan\nmixrampdelay: nan")).mixramp,
        MixRamp::Unknown
    );
}
#[test]
fn current_status_metadata_is_parsed_without_changing_playback_values() {
    let source = lines("state: play\naudio: 96000:24:2\nvolume: 43\nxfade: 0\nmixrampdelay: nan");
    let before = source.clone();
    let status = ProcessingStatus::from_status(&source);
    assert_eq!(status.mpd_volume, Some(43));
    assert_eq!(status.crossfade_seconds, Some(0.0));
    assert_eq!(status.mixramp, MixRamp::Off);
    assert_eq!(source, before);
    let enabled = ProcessingStatus::from_status(&lines("volume: 100\nxfade: 2.5\nmixrampdelay: 0.5"));
    assert_eq!(enabled.crossfade_seconds, Some(2.5));
    assert_eq!(enabled.mixramp, MixRamp::Possible);
    assert!(!serde_json::to_string(&enabled).unwrap().contains("NaN"));
}
#[test]
fn replaygain_modes_are_allowlisted_and_duplicates_fail_closed() {
    for (value, expected) in [
        ("off", ReplayGain::Off),
        ("track", ReplayGain::Track),
        ("album", ReplayGain::Album),
        ("auto", ReplayGain::Auto),
    ] {
        assert_eq!(
            parse_mode(&lines(&format!("replay_gain_mode: {value}"))),
            Ok(expected)
        );
    }
    for source in [
        "",
        "replay_gain_mode: unknown",
        "replay_gain_mode: off\nreplay_gain_mode: off",
    ] {
        assert_eq!(parse_mode(&lines(source)), Err(ReadFailure::Protocol));
    }
}
fn exchange(response: &'static [u8]) -> (Result<ReplayGain, ReadFailure>, Vec<String>) {
    let dir = fixture();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"OK MPD 0.24.15\n").unwrap();
        let mut command = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut command)
            .unwrap();
        let _ = stream.write_all(response);
        vec![command]
    });
    let result = query(&path, std::process::id());
    let calls = server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
    (result, calls)
}
#[test]
fn owned_fake_mpd_gets_only_one_readonly_command_and_no_transport_or_config_mutations() {
    for (response, mode) in [
        (b"replay_gain_mode: off\nOK\n".as_slice(), ReplayGain::Off),
        (
            b"replay_gain_mode: album\nOK\n".as_slice(),
            ReplayGain::Album,
        ),
    ] {
        let (result, calls) = exchange(response);
        assert_eq!(result, Ok(mode));
        assert_eq!(calls, ["replay_gain_status\n"]);
    }
}
#[test]
fn ack_eof_partial_and_oversized_responses_are_not_success_and_never_leak_details() {
    for response in [
        b"ACK [5@0] {replay_gain_status} private-token\n".as_slice(),
        b"replay_gain_mode: off\n",
        b"replay_gain_mode: off\nOK",
        b"OK\n",
        b"replay_gain_mode: off\nreplay_gain_mode: album\nOK\n",
    ] {
        let (result, calls) = exchange(response);
        assert!(result.is_err());
        assert_eq!(calls, ["replay_gain_status\n"]);
        assert!(!format!("{result:?}").contains("private-token"));
    }
    let dir = fixture();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"OK MPD 0.24.15\n").unwrap();
        let mut cmd = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut cmd)
            .unwrap();
        let _ = stream.write_all(&vec![b'x'; 8192]);
    });
    assert_eq!(query(&path, std::process::id()), Err(ReadFailure::Protocol));
    server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
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
    assert_eq!(
        query(&path, std::process::id() + 1),
        Err(ReadFailure::Changed)
    );
    assert!(server.join().unwrap().is_empty());
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unresponsive_peer_is_bounded_and_missing_socket_is_unavailable() {
    let dir = fixture();
    let path = dir.join("s");
    assert_eq!(
        query(&path, std::process::id()),
        Err(ReadFailure::Unavailable)
    );
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(220));
    });
    let started = Instant::now();
    assert_eq!(query(&path, std::process::id()), Err(ReadFailure::Timeout));
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
    assert_eq!(query(&path, std::process::id()), Err(ReadFailure::Timeout));
    assert!(started.elapsed() < Duration::from_millis(450));
    server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn complete_mpd_status_omits_disabled_crossfade_and_mixramp() {
    // PlayerCommands.cxx::handle_status prints each conditional field only >0.
    let envelope = "repeat: 0\nrandom: 0\nsingle: 0\nconsume: 0\nplaylist: 1\nplaylistlength: 2\nmixrampdb: 0\nstate: play\nvolume: 100\naudio: 96000:24:2";
    let source = lines(envelope);
    let before = source.clone();
    let status = ProcessingStatus::from_status(&source);
    assert_eq!(status.crossfade_seconds, Some(0.0));
    assert_eq!(status.mixramp, MixRamp::Off);
    assert_eq!(source, before);
    for state in ["pause", "stop"] {
        let status = ProcessingStatus::from_status(&lines(&envelope.replace("state: play", &format!("state: {state}"))));
        assert_eq!(status.crossfade_seconds, Some(0.0));
        assert_eq!(status.mixramp, MixRamp::Off);
    }
    let enabled = ProcessingStatus::from_status(&lines(&format!("{envelope}\nxfade: 3\nmixrampdelay: 0.5")));
    assert_eq!(enabled.crossfade_seconds, Some(3.0));
    assert_eq!(enabled.mixramp, MixRamp::Possible);
    for value in ["0", "-1", "nan"] {
        assert_eq!(ProcessingStatus::from_status(&lines(&format!("{envelope}\nmixrampdelay: {value}"))).mixramp, MixRamp::Off);
    }
    for bad in ["xfade: nan", "xfade: 0\nxfade: 0", "xfade: invalid", "xfade:0", "xfade: -1"] {
        assert_eq!(ProcessingStatus::from_status(&lines(&format!("{envelope}\n{bad}"))).crossfade_seconds, None);
    }
    for bad in ["mixrampdelay: inf", "mixrampdelay: 1\nmixrampdelay: 1", "mixrampdelay: invalid", "mixrampdelay:0"] {
        assert_eq!(ProcessingStatus::from_status(&lines(&format!("{envelope}\n{bad}"))).mixramp, MixRamp::Unknown);
    }
    for required in ["state", "repeat", "random", "single", "consume", "playlist", "playlistlength", "mixrampdb"] {
        let partial: Vec<_> = source.iter().filter(|line| !line.starts_with(&format!("{required}:"))).cloned().collect();
        let status = ProcessingStatus::from_status(&partial);
        assert_eq!(status.crossfade_seconds, None, "{required}");
        assert_eq!(status.mixramp, MixRamp::Unknown, "{required}");
    }
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
    let result = query_lines(&path, std::process::id(), true).unwrap();
    assert_eq!(parse_transport(&result).unwrap().kind, TransportKind::Dop);
    assert_eq!(server.join().unwrap(), "outputs\n");
    fs::remove_dir_all(dir).unwrap();
}
