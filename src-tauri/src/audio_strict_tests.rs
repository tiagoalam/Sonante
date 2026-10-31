use super::*;
use std::cell::Cell;

fn fixture() -> AudioEngine {
    let mut engine = super::reliability_tests::fixture();
    engine.queue[0].uri = "a.flac".into();
    engine.queue[0].media_locator = Some(MediaLocator::Local { uri: "a.flac".into() });
    let mut second = engine.queue[0].clone();
    second.uri = "b.flac".into(); second.title = "B".into();
    second.media_locator = Some(MediaLocator::Local { uri: "b.flac".into() });
    engine.queue.push(second);
    engine
}

#[test]
fn strict_regression_failed_recovery_fences_queue_after_owner_release() {
    for state in [PlaybackState::Playing, PlaybackState::Paused, PlaybackState::Stopped] {
        for restart_ok in [true, false] {
            let mut engine = fixture();
            let before = serde_json::to_string(&engine.queue).unwrap();
            let resolved = engine.capture_device_switch_queue().unwrap()
                .bind_resolved_uris(vec!["a.flac".into(), "b.flac".into()]).unwrap();
            let snapshot = Some(DeviceSwitchSnapshot { queue_index: 1, elapsed: 30.0, state });
            let selection = if state == PlaybackState::Stopped { "" } else { "play 1\n" };
            let batch = format!("command_list_begin\nclear\nadd \"a.flac\"\nadd \"b.flac\"\n{selection}command_list_end");
            let mut script = vec![(batch.clone(), Some("ACK [50@2] {add} unavailable\n".into()))];
            if restart_ok {
                let stopped_batch = "command_list_begin\nclear\nadd \"a.flac\"\nadd \"b.flac\"\ncommand_list_end";
                script.push((stopped_batch.into(), Some("ACK [50@2] {add} unavailable\n".into())));
            }
            let (socket, server) = super::tests::fake_mpd_script(script);
            engine.socket_path = socket;
            assert!(engine.restore_after_reserved_device_switch(snapshot, &resolved).is_err());
            let audio = Mutex::new(engine);
            assert!(crate::rollback_audio_switch_with(snapshot, &resolved, &audio,
                || if restart_ok { Ok(()) } else { Err("restart failed".into()) }).is_err());
            drop(resolved);
            let mut engine = audio.lock().unwrap();
            assert_eq!(engine.queue_structure_reserved.load(Ordering::SeqCst), 0);
            assert!(!engine.queue_coherent, "failed recovery cannot prove the MPD queue");
            assert_eq!(serde_json::to_string(&engine.queue).unwrap(), before, "preserve last valid cache metadata");
            assert!(engine.get_queue().is_empty());
            assert!(engine.current_media_for_queue_index(Some(0)).is_none());
            assert!(engine.persist_shutdown_resume().is_err());
            assert!(engine.play_index(0, false).is_err());
            let tracks = engine.queue.clone();
            engine.play_tracks_with_io(tracks, vec!["a.flac".into(), "b.flac".into()], 1,
                |_| Ok(vec![]), |_| Ok(())).unwrap();
            assert!(engine.queue_coherent);
            assert_eq!(engine.get_queue().len(), 2);
            server.join().unwrap();
        }
    }
}

#[test]
fn strict_regression_partial_position_is_distinct_from_complete_success() {
    let engine = fixture();
    let mut commands = Vec::new();
    let result = engine.restore_after_device_switch_with(
        Some(DeviceSwitchSnapshot { queue_index: 1, elapsed: 30.0, state: PlaybackState::Paused }),
        &["a.flac".into(), "b.flac".into()], &mut |command| {
            commands.push(command.to_string());
            if command.starts_with("seekcur") { Err("ACK [5@0] {seekcur} Not seekable".into()) }
            else { Ok(vec![]) }
        }, &mut |_| {});
    assert_eq!(format!("{:?}", result.unwrap()), "PositionNotRestored");
    assert_eq!(commands.iter().filter(|c| c.starts_with("seekcur")).count(), 3);
    assert_eq!(commands.iter().filter(|c| c.starts_with("command_list")).count(), 1);
    assert_eq!(commands.last().unwrap(), "pause 1");
}

#[test]
fn strict_regression_contradictory_track_cannot_publish_metadata_or_format() {
    let engine = fixture();
    for (status_id, song_id, pos) in [(7, 8, 1), (7, 8, 0), (7, 7, 1)] {
        let calls = Cell::new(0);
        let status = engine.get_status_with_io(true, true, |_| panic!("no cover lookup for a mixed query"), |command| {
            calls.set(calls.get()+1);
            Ok(match command {
                "status" => vec!["state: play".into(), "song: 0".into(), format!("songid: {status_id}"), "elapsed: 40".into(), "duration: 120".into(), "audio: 48000:16:2".into()],
                "currentsong" => vec![format!("Pos: {pos}"), format!("Id: {song_id}"), "file: b.flac".into(), "Title: B".into()],
                _ => panic!("extra query"),
            })
        }).unwrap();
        assert!(status.title.is_empty() && status.artist.is_empty() && status.album.is_empty());
        assert!(status.current_media.is_none() && status.thumb.is_none() && status.plex_image.is_none());
        assert!(status.audio_format.is_empty() && status.processing.is_none());
        assert_eq!((status.elapsed,status.duration),(0.0,0.0));
        assert_eq!(serde_json::to_value(status).unwrap()["snapshot_consistent"], false);
        assert_eq!(calls.get(),2);
    }
    let recovered = engine.get_status_with_io(false,false,|_|None,|cmd| Ok(if cmd=="status" {
        vec!["state: play".into(), "song: 1".into(), "songid: 8".into(), "audio: 96000:24:2".into()]
    } else { vec!["Pos: 1".into(), "Id: 8".into(), "file: b.flac".into()] })).unwrap();
    assert_eq!(recovered.title,"B");
    assert_eq!(recovered.audio_format,"96000:24:2");
    assert!(recovered.current_media.is_some());
}

#[test]
fn partial_restore_caller_publishes_warning_without_rollback_or_extra_play() {
    for state in [PlaybackState::Playing, PlaybackState::Paused, PlaybackState::Stopped] {
        for seek_case in ["accepted", "not_seekable", "other_ack", "retry"] {
            let engine = fixture();
            let mut commands = Vec::new();
            let mut seeks = 0;
            let result = engine.restore_after_device_switch_with(
                Some(DeviceSwitchSnapshot { queue_index: 1, elapsed: 30.0, state }),
                &["a.flac".into(), "b.flac".into()], &mut |command| {
                    commands.push(command.to_string());
                    if command.starts_with("seekcur") {
                        seeks += 1;
                        if seek_case == "not_seekable" || (seek_case == "retry" && seeks < 3) {
                            return Err("ACK [5@0] {seekcur} Not seekable".into());
                        }
                        if seek_case == "other_ack" { return Err("ACK [50@0] {seekcur} unavailable".into()); }
                    }
                    Ok(vec![])
                }, &mut |_| {});
            let rollbacks = Cell::new(0);
            let applied = crate::finish_device_switch_restore(result, || { rollbacks.set(rollbacks.get()+1); Ok(()) });
            let stopped = state == PlaybackState::Stopped;
            assert_eq!(rollbacks.get(), u32::from(!stopped && seek_case == "other_ack"));
            if applied.is_ok() {
                let response = crate::config_save_operation::SaveResponse::completed(applied.unwrap());
                let json = serde_json::to_value(response).unwrap();
                assert_eq!(json["status"], "completed");
                assert_eq!(json.get("warning").is_some(), !stopped && seek_case == "not_seekable");
                if !stopped && seek_case == "not_seekable" { assert_eq!(json["warning"], "position_not_restored"); }
            }
            assert_eq!(commands.iter().filter(|s| s.starts_with("command_list_begin")).count(),1);
            if stopped { assert_eq!(commands.len(),1); }
            else if seek_case != "other_ack" { assert_eq!(commands.last().unwrap(),"pause 1"); }
        }
    }
}

#[test]
fn recovery_proved_by_same_owner_stages_paused_cursor_and_failed_queue_fences_it() {
    for failure in [false, true] {
        let mut engine = fixture();
        let resolved = engine.capture_device_switch_queue().unwrap()
            .bind_resolved_uris(vec!["a.flac".into(), "b.flac".into()]).unwrap();
        let snapshot = Some(DeviceSwitchSnapshot { queue_index: 1, elapsed: 30.0, state: PlaybackState::Paused });
        // Rollback stages the queue without opening an output. The fake server
        // accepts exactly one batch, so Play/Seek/Pause cannot satisfy this test.
        let batch = "command_list_begin\nclear\nadd \"a.flac\"\nadd \"b.flac\"\ncommand_list_end";
        let script = vec![(batch.into(), Some(if failure {
            "ACK [50@2] {add} unavailable\n"
        } else { "OK\n" }.into()))];
        let (socket,server)=super::tests::fake_mpd_script(script);engine.socket_path=socket;
        let audio=Mutex::new(engine);
        let result=crate::rollback_audio_switch_with(snapshot,&resolved,&audio,||Ok(()));
        assert_eq!(result.is_ok(),!failure);drop(resolved);
        let engine=audio.lock().unwrap();assert_eq!(engine.queue_coherent,!failure);
        assert_eq!(engine.get_queue().is_empty(),failure);
        if !failure {
            assert_eq!(engine.restored_resume, Some(PlaybackResumeSnapshot {
                queue_index: 1, elapsed: 30.0, state: PlaybackState::Paused,
            }));
        }
        assert_eq!(engine.queue_structure_reserved.load(Ordering::SeqCst),0);server.join().unwrap();
    }
}

#[test]
fn unsupported_format_diagnostic_is_allowlisted_and_never_exposes_server_data() {
    let error = "Failed to open private://SECRET?X-Plex-Token=SECRET: Sonante Direct strict: unsupported sample rate";
    assert_eq!(serde_json::to_string(&PlaybackDiagnostic::from_mpd_error(error)).unwrap(),r#"{"code":"format_unsupported"}"#);
}

#[test]
fn strict_diagnostic_requires_a_known_terminal_reason_and_does_not_expose_source_text() {
    for reason in ["unsupported sample format", "unsupported channel count", "unsupported sample rate",
        "forced output format is incompatible", "allowed_formats is incompatible"] {
        for context in ["", "Failed to open Sonante Output: "] {
            let raw = format!("{context}Sonante Direct strict: {reason}");
            assert_eq!(PlaybackDiagnostic::from_mpd_error(&raw), PlaybackDiagnostic::FormatUnsupported);
        }
    }
    for raw in [
        "Failed to open https://private.invalid/live?token=SECRET: HTTP 404 Not Found",
        "Sonante Direct strict: connection refused https://private.invalid/?token=SECRET",
        "Failed to decode Sonante Direct strict: unsupported sample rate in source name",
        "Failed to open https://private.invalid/Sonante Direct strict: unsupported sample rate",
    ] {
        let code=PlaybackDiagnostic::from_mpd_error(raw);
        assert_eq!(code, PlaybackDiagnostic::PlaybackError);
        assert_eq!(serde_json::to_string(&code).unwrap(), r#"{"code":"playback_error"}"#);
    }
}
