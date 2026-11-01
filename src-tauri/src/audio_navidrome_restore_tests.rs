//! Own-memory/cache and protocol-peer fixtures; never launches MPD or opens audio.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicU64, Ordering};

fn track(title: &str, locator: MediaLocator) -> TrackMetadata {
    TrackMetadata {
        title: title.into(),
        artist: "Artist".into(),
        album: "Album".into(),
        uri: match &locator {
            MediaLocator::Local { uri } => uri.clone(),
            _ => String::new(),
        },
        media_locator: Some(locator),
        duration: Some(120.0),
        thumb: None,
        plex_image: None,
    }
}
fn nav(session: &str, song: &str) -> TrackMetadata {
    track(
        "Navidrome",
        MediaLocator::Navidrome {
            connection_id: session.into(),
            account_id: Some("a".repeat(64)),
            song_id: song.into(),
        },
    )
}
fn local() -> TrackMetadata {
    track(
        "Local",
        MediaLocator::Local {
            uri: "Álbum/uma \\\"faixa\\\".flac".into(),
        },
    )
}
fn plex() -> TrackMetadata {
    track(
        "Plex",
        MediaLocator::Plex {
            server_id: "fixture-server".into(),
            part_key: "/library/parts/42/file.flac".into(),
            file_path: None,
            rating_key: Some("track-42".into()),
        },
    )
}
fn engine(queue: Vec<TrackMetadata>) -> AudioEngine {
    AudioEngine {
        socket_path: "/no-fixture-peer".into(),
        music_dir: String::new(),
        queue,
        restored_resume: None,
        startup_restore: StartupQueueRestore::Pending,
        queue_structure_reserved: Arc::new(AtomicU8::new(0)),
        queue_coherent: true,
        cache_persistence_pending: false,
        playback_intent: 0,
        eq_intervention: Arc::new(AtomicU64::new(0)),
        direct_playback: None,
    }
}
/// Captures the complete batch with bounded waits, including simulated ACK/EOF.
fn peer(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "sn-navq-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let socket = path.to_string_lossy().into_owned();
    let task = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) => panic!("fixture peer unavailable: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(b"OK MPD 0.23.5\n").unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut batch = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            batch.push_str(&line);
            if line == "command_list_end\n" {
                break;
            }
        }
        if !reply.is_empty() {
            stream.write_all(reply.as_bytes()).unwrap();
        }
        std::fs::remove_file(path).unwrap();
        batch
    });
    (socket, task)
}

#[test]
fn durable_cache_roundtrip_keeps_navidrome_order_duplicates_and_cursor_without_proxy_secrets() {
    let mut first = nav("PRIVATE_OLD_SESSION", "song-a");
    first.uri = "http://127.0.0.1:1234/nav/PRIVATE_PROXY/song?password=PRIVATE_PASSWORD".into();
    first.thumb = Some("data:image/png;base64,PRIVATE_ARTWORK".into());
    let queue = vec![
        local(),
        first.clone(),
        plex(),
        nav("PRIVATE_OLD_SESSION", "song-b"),
        first,
    ];
    for state in [PlaybackState::Playing, PlaybackState::Paused] {
        let cursor = PlaybackResumeSnapshot {
            queue_index: 4,
            elapsed: 61.25,
            state,
        };
        let bytes = AudioEngine::encode_queue_cache_with_resume(&queue, Some(&cursor)).unwrap();
        let safe = String::from_utf8(bytes.clone()).unwrap();
        for secret in ["PRIVATE_", "127.0.0.1", "password", "data:image"] {
            assert!(!safe.contains(secret));
        }
        let (loaded, resume, changed) =
            AudioEngine::decode_queue_cache_with_resume(bytes.as_slice()).unwrap();
        assert!(!changed);
        assert_eq!(loaded.len(), 5);
        assert_eq!(resume, Some(cursor));
        assert_eq!(loaded[1].media_locator, loaded[4].media_locator);
        assert_ne!(loaded[1].media_locator, loaded[3].media_locator);
        assert_eq!(loaded[0].media_locator, queue[0].media_locator);
        assert_eq!(loaded[2].media_locator, queue[2].media_locator);
        assert_eq!(
            AudioEngine::encode_queue_cache_with_resume(&loaded, resume.as_ref()).unwrap(),
            bytes
        );
        let audio = engine(queue.clone());
        crate::recovery_set::validate_queue_capture(Some(&bytes), &audio.durable_queue_tracks())
            .unwrap();
    }
}

#[test]
fn transient_stream_filter_remaps_occurrence_cursor_and_never_falls_back_to_a_different_track() {
    let live = track(
        "Radio",
        MediaLocator::Radio {
            station_id: "fixture-radio".into(),
        },
    );
    let queue = vec![
        live.clone(),
        nav("session", "repeated"),
        local(),
        nav("session", "repeated"),
        live,
    ];
    for (index, expected) in [
        (0, None),
        (1, Some(0)),
        (2, Some(1)),
        (3, Some(2)),
        (4, None),
    ] {
        let cursor = PlaybackResumeSnapshot {
            queue_index: index,
            elapsed: 15.,
            state: PlaybackState::Paused,
        };
        let bytes = AudioEngine::encode_queue_cache_with_resume(&queue, Some(&cursor)).unwrap();
        let (loaded, restored, _) =
            AudioEngine::decode_queue_cache_with_resume(bytes.as_slice()).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(restored.map(|cursor| cursor.queue_index), expected);
    }
}

#[test]
fn malformed_or_session_only_navidrome_entries_never_infer_an_account_or_shift_resume() {
    let mut invalid = nav("PRIVATE_SESSION", "song-a");
    invalid.media_locator = Some(MediaLocator::Navidrome {
        connection_id: "PRIVATE_SESSION".into(),
        account_id: None,
        song_id: "song-a".into(),
    });
    let cursor = PlaybackResumeSnapshot {
        queue_index: 1,
        elapsed: 33.,
        state: PlaybackState::Paused,
    };
    for malformed in [invalid.clone(), nav("session", "../escape"), {
        let mut wrong = nav("session", "song-a");
        if let Some(MediaLocator::Navidrome { account_id, .. }) = &mut wrong.media_locator {
            *account_id = Some("not-an-account".into());
        }
        wrong
    }] {
        // Legacy shape is accepted only as input; output normalizes valid durable identities.
        let old = serde_json::json!({"version":2,"tracks":[malformed,nav("PRIVATE_SESSION","good")],"resume":cursor});
        let (loaded, restored, changed) = AudioEngine::decode_queue_cache_with_resume(
            serde_json::to_vec(&old).unwrap().as_slice(),
        )
        .unwrap();
        assert!(changed);
        assert_eq!(loaded.len(), 1);
        assert_eq!(restored.unwrap().queue_index, 0);
        assert_eq!(restored.unwrap().elapsed, 33.);
        let mut old = old;
        old["resume"]["queue_index"] = serde_json::json!(0);
        let (_, restored, _) = AudioEngine::decode_queue_cache_with_resume(
            serde_json::to_vec(&old).unwrap().as_slice(),
        )
        .unwrap();
        assert!(
            restored.is_none(),
            "discarded occurrence must not select its surviving neighbor"
        );
    }
    assert!(engine(vec![invalid]).durable_queue_tracks().is_empty());
}

#[test]
fn startup_restores_mixed_queue_only_after_ack_and_never_plays_seeks_or_changes_the_cursor() {
    let captured = vec![local(), nav("", "song-a"), plex(), nav("", "song-a")];
    let mut rebound = captured.clone();
    for track in &mut rebound {
        if let Some(MediaLocator::Navidrome { connection_id, .. }) = &mut track.media_locator {
            *connection_id = "NEW_SESSION".into();
        }
    }
    let uris = vec![
        captured[0].uri.clone(),
        "http://127.0.0.1:1234/nav/NEW_PROXY/song-a".into(),
        "https://fixture.invalid/part?X-Plex-Token=PRIVATE".into(),
        "http://127.0.0.1:1234/nav/NEW_PROXY/song-a".into(),
    ];
    for state in [
        None,
        Some(PlaybackState::Playing),
        Some(PlaybackState::Paused),
    ] {
        let mut audio = engine(captured.clone());
        audio.restored_resume = state.map(|state| PlaybackResumeSnapshot {
            queue_index: 3,
            elapsed: 61.25,
            state,
        });
        let before = audio.restored_resume;
        let (socket, server) = peer("OK\n");
        audio.socket_path = socket;
        audio
            .restore_startup_queue_references(&captured, rebound.clone(), &uris)
            .unwrap();
        assert_eq!(audio.startup_restore, StartupQueueRestore::Ready);
        assert_eq!(audio.restored_resume, before);
        assert_eq!(audio.queue[3].media_locator, rebound[3].media_locator);
        let batch = server.join().unwrap();
        assert_eq!(
            batch
                .lines()
                .filter(|line| line.starts_with("add "))
                .count(),
            4
        );
        assert!(batch.lines().all(|line| line == "command_list_begin"
            || line == "clear"
            || line == "command_list_end"
            || line.starts_with("add ")));
        let bytes = AudioEngine::encode_queue_cache_with_resume(
            &audio.queue,
            audio.restored_resume.as_ref(),
        )
        .unwrap();
        let safe = String::from_utf8(bytes).unwrap();
        assert!(!safe.contains("PRIVATE"));
        assert!(!safe.contains("NEW_SESSION"));
        assert!(!safe.contains("NEW_PROXY"));
    }
}

#[test]
fn restore_ack_eof_and_transport_failures_keep_saved_references_and_block_unconfirmed_playback() {
    let saved = vec![nav("", "song-a")];
    let mut rebound = saved.clone();
    if let Some(MediaLocator::Navidrome { connection_id, .. }) = &mut rebound[0].media_locator {
        *connection_id = "new-session".into();
    }
    for reply in [Some("ACK [50@1] {add} refused\n"), Some(""), None] {
        let mut audio = engine(saved.clone());
        let server = reply.map(|reply| {
            let (socket, server) = peer(reply);
            audio.socket_path = socket;
            server
        });
        assert!(audio
            .restore_startup_queue_references(
                &saved,
                rebound.clone(),
                &["http://127.0.0.1:1234/nav/private/song-a".into()]
            )
            .is_err());
        assert_eq!(audio.queue[0].media_locator, saved[0].media_locator);
        assert_eq!(audio.startup_restore, StartupQueueRestore::Failed);
        assert!(audio.play_index(0, false).is_err());
        if let Some(server) = server {
            server.join().unwrap();
        }
    }
}

#[test]
fn stale_or_cross_account_restore_is_refused_before_socket_effects_and_newer_playback_wins() {
    let saved = vec![nav("", "song-a")];
    for invalid in [vec![nav("new-session", "different-song")], {
        let mut wrong = nav("new-session", "song-a");
        if let Some(MediaLocator::Navidrome { account_id, .. }) = &mut wrong.media_locator {
            *account_id = Some("b".repeat(64));
        }
        vec![wrong]
    }] {
        let mut audio = engine(saved.clone());
        assert_eq!(
            audio
                .restore_startup_queue_references(&saved, invalid, &["url".into()])
                .unwrap_err(),
            "STARTUP_QUEUE_CONTEXT_CHANGED"
        );
        assert_eq!(audio.queue[0].media_locator, saved[0].media_locator);
    }
    let mut audio = engine(vec![local()]);
    assert_eq!(
        audio
            .restore_startup_queue_references(&saved, saved.clone(), &["url".into()])
            .unwrap_err(),
        "STARTUP_QUEUE_CONTEXT_CHANGED"
    );
    audio.startup_restore = StartupQueueRestore::Ready;
    audio
        .restore_startup_queue_references(&saved, saved.clone(), &["url".into()])
        .unwrap();
    assert_eq!(audio.queue[0].media_locator, local().media_locator);
}

#[test]
fn recovery_proof_accepts_only_canonical_navidrome_account_song_identity_and_exact_order() {
    let queue = vec![
        nav("live-session", "one"),
        local(),
        nav("live-session", "two"),
    ];
    let bytes = AudioEngine::encode_queue_cache(&queue).unwrap();
    let canonical = engine(queue.clone()).durable_queue_tracks();
    crate::recovery_set::validate_queue_capture(Some(&bytes), &canonical).unwrap();
    let mut wrong = canonical.clone();
    wrong.swap(0, 2);
    assert!(crate::recovery_set::validate_queue_capture(Some(&bytes), &wrong).is_err());
    assert!(
        crate::recovery_set::validate_queue_capture(Some(&bytes), &queue).is_err(),
        "raw live sessions are not a durable proof"
    );
    let mut wrong = canonical;
    wrong[0].uri = "http://127.0.0.1/PRIVATE".into();
    assert!(crate::recovery_set::validate_queue_capture(Some(&bytes), &wrong).is_err());
}

#[test]
fn cache_load_keeps_saved_bytes_and_reopened_navidrome_status_returns_paused_at_the_exact_occurrence(
) {
    let queue = vec![
        nav("OLD_SESSION", "duplicate"),
        local(),
        nav("OLD_SESSION", "duplicate"),
    ];
    let cursor = PlaybackResumeSnapshot {
        queue_index: 2,
        elapsed: 31.25,
        state: PlaybackState::Playing,
    };
    let bytes = AudioEngine::encode_queue_cache_with_resume(&queue, Some(&cursor)).unwrap();
    let path = get_queue_cache_path().unwrap();
    assert!(!path.exists());
    crate::persistence::atomic_write_private(&path, &bytes, "queue_cache.json").unwrap();
    let mut audio = AudioEngine::new("/no-peer", "/no-music");
    assert!(audio.startup_queue_pending());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "valid load must not rewrite the durable cache"
    );
    let captured = audio.queue.clone();
    let mut rebound = captured.clone();
    for track in &mut rebound {
        if let Some(MediaLocator::Navidrome { connection_id, .. }) = &mut track.media_locator {
            *connection_id = "NEW_SESSION".into();
        }
    }
    let uris = vec![
        "http://127.0.0.1:1234/nav/new/duplicate".into(),
        captured[1].uri.clone(),
        "http://127.0.0.1:1234/nav/new/duplicate".into(),
    ];
    let (socket, server) = peer("OK\n");
    audio.socket_path = socket;
    audio
        .restore_startup_queue_references(&captured, rebound, &uris)
        .unwrap();
    server.join().unwrap();
    let status = audio
        .get_status_with_io(
            true,
            true,
            |_| panic!("no local artwork for a Navidrome cursor"),
            |command| {
                assert_eq!(
                    command, "status",
                    "restored telemetry must never issue play/seek or inspect the server"
                );
                Ok(vec!["state: stop".into()])
            },
        )
        .unwrap();
    assert_eq!(status.state, "pause");
    assert_eq!(status.elapsed, 31.25);
    assert_eq!(
        status.current_media,
        Some(CurrentMedia::Navidrome {
            connection_id: "NEW_SESSION".into(),
            song_id: "duplicate".into(),
            queue_index: 2
        })
    );
    assert_eq!(status.title, "Navidrome");
    assert_eq!(status.duration, 120.);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "runtime session must not be written during restore"
    );
    std::fs::remove_file(path).unwrap();
}
