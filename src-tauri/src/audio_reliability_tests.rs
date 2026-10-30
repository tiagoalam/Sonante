use super::*;
pub(crate) fn fixture() -> AudioEngine { AudioEngine {
 socket_path:String::new(),music_dir:String::new(),queue:vec![TrackMetadata{title:"Old".into(),artist:"Fixture".into(),album:"Fixture".into(),thumb:Some("fixture-cover".into()),plex_image:None,media_locator:None,uri:"old.flac".into(),duration:Some(120.0)}],restored_resume:None,startup_restore:StartupQueueRestore::Ready,queue_structure_reserved:Arc::new(AtomicU8::new(0)),queue_coherent:true,cache_persistence_pending:false,playback_intent:0,eq_intervention:Arc::new(AtomicU64::new(0)),direct_playback:None
} }
#[test]
fn partial_queue_failure_fences_actual_engine() {
 let mut engine=fixture(); let tracks=engine.queue.clone();
 let error=engine.play_tracks_with_io(tracks,vec!["new.flac".into()],0,
    |_|Err("ACK [50@1] {add} rejected".into()), |_|panic!("must not persist rejected queue")).unwrap_err();
 assert_eq!(error,"PLAYBACK_QUEUE_UNCERTAIN");
 assert!(engine.play_index(0,false).is_err());
 assert!(engine.persist_shutdown_resume().is_err());
 assert!(engine.current_media_for_queue_index(Some(0)).is_none());
 assert!(engine.get_queue().is_empty());
}

#[test]
fn command_list_effects_never_publish_old_identity_or_shutdown_cursor() {
 use std::cell::RefCell;
 for failed_at in ["add", "play", "reply"] {
  let mut e=fixture(); let tracks=vec![e.queue[0].clone(),e.queue[0].clone()];
  let real=RefCell::new(vec!["old.flac".to_string()]);
  let result=e.play_tracks_with_io(tracks,vec!["new.flac".into(),"new.flac".into()],0,|batch| {
   for line in batch.lines() {
    if line=="clear" { real.borrow_mut().clear(); }
    if line.starts_with("add ") {
     if failed_at=="add" && real.borrow().len()==1 { return Err("ACK [50@2] {add} unavailable".into()); }
     real.borrow_mut().push("new.flac".into());
    }
    if line.starts_with("play ") && failed_at=="play" {return Err("ACK [50@3] {play} refused".into());}
   }
   Err("EOF after all commands executed".into())
  }, |_|panic!("partial effects must not write cache"));
  assert_eq!(result.unwrap_err(),"PLAYBACK_QUEUE_UNCERTAIN");
  assert!(!real.borrow().is_empty());
  assert!(e.restored_resume.is_none());assert!(e.current_media_for_queue_index(Some(0)).is_none());
  assert!(e.capture_device_switch_queue().is_err());assert!(e.play_index(0,false).is_err());
  assert!(e.seek(1.0,false).is_err());assert!(e.persist_shutdown_resume().is_err());
  let status=e.get_status_with_io(true,true,|_|panic!("telemetry must not read artwork"), |cmd|Ok(if cmd=="status" { vec!["state: play".into(),"song: 0".into(),"audio: 96000:24:2".into(),"volume: 100".into()] } else { vec!["file: new.flac".into(),"Pos: 0".into(),"Title: New".into()] })).unwrap();
  assert_eq!(status.title,"New");assert!(status.current_media.is_none());assert!(status.processing.is_none());
  assert_eq!(status.playback_error,Some(PlaybackDiagnostic::QueueUncertain));
  // An explicit new selection, including duplicate occurrences, recovers.
  let tracks=vec![e.queue[0].clone(),e.queue[0].clone()];
  e.play_tracks_with_io(tracks,vec!["new.flac".into(),"new.flac".into()],1, |_|Ok(vec![]), |_|Ok(())).unwrap();
  assert!(e.queue_coherent);assert_eq!(e.get_queue().len(),2);assert!(e.current_media_for_queue_index(Some(1)).is_some());
 }
}

#[test]
fn persistence_failure_after_ok_is_distinct_and_never_overwrites_saved_resume() {
 let mut e=fixture(); let mut replacement=e.queue[0].clone();replacement.title="New".into();replacement.uri="new.flac".into();
 let result=e.play_tracks_with_io(vec![replacement],vec!["new.flac".into()],0, |_|Ok(vec![]), |_|Err("EACCES".into()));
 assert_eq!(result.unwrap_err(),"PLAYBACK_PERSISTENCE_FAILED");assert!(e.queue_coherent);assert!(e.cache_persistence_pending);
 assert_eq!(e.get_queue()[0].title,"New");
 // Empty socket ensures this must fail before any capture/status or disk I/O.
 assert_eq!(e.persist_shutdown_resume().unwrap_err(),"PLAYBACK_PERSISTENCE_FAILED");
}

#[test]
fn latest_admitted_backend_intent_survives_old_failure_cancel_and_switch() {
 let mut e=fixture();let old=e.admit_playback_intent().unwrap();let new=e.admit_playback_intent().unwrap();
 assert_eq!(e.validate_playback_intent(&old).unwrap_err(),"PLAYBACK_SUPERSEDED");
 e.cancel_playback_intent(&old);assert!(e.validate_playback_intent(&new).is_ok());
 e.invalidate_playback_intents();assert!(e.validate_playback_intent(&new).is_err());
 let token=e.admit_playback_intent().unwrap();let _switch=e.capture_device_switch_queue().unwrap();
 assert!(e.validate_playback_intent(&token).is_err());assert!(e.admit_playback_intent().is_err());
}

#[test]
fn textual_validation_happens_before_any_effect_and_tokens_do_not_cross_errors() {
 let mut e=fixture();let tracks=e.queue.clone();
 assert!(e.play_tracks_with_io(tracks,vec!["invalid\nclear".into()],0, |_|panic!("invalid argument must not send"), |_|panic!()).is_err());
 assert!(e.queue_coherent);
 let tracks=e.queue.clone();
 let error=e.play_tracks_with_io(tracks,vec!["https://fixture.invalid/part?X-Plex-Token=private".into()],0, |_|Err("ACK token private".into()), |_|panic!()).unwrap_err();
 assert_eq!(error,"PLAYBACK_QUEUE_UNCERTAIN");assert!(!error.contains("private"));
}

#[test]
fn uncertain_queue_allows_pause_and_mute_but_never_resumes() {
 use std::{io::{BufRead,BufReader,Write},os::unix::net::UnixListener};
 let dir=std::env::temp_dir().join(format!("sonante-queue-fence-{}",std::process::id()));
 std::fs::create_dir(&dir).unwrap();let path=dir.join("mpd.sock");
 let listener=UnixListener::bind(&path).unwrap();
 let server=std::thread::spawn(move || {
  for (expected,response) in [("pause 1","OK\n"),("status","state: play\nOK\n"),("pause 1","OK\n"),("setvol 0","OK\n"),("status","state: pause\nOK\n")] {
   let (mut stream,_)=listener.accept().unwrap();stream.write_all(b"OK MPD 0.24.15\n").unwrap();
   let mut line=String::new();BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();assert_eq!(line.trim_end(),expected);stream.write_all(response.as_bytes()).unwrap();
  }
 });
 let mut e=fixture();e.socket_path=path.to_string_lossy().into();e.mark_queue_uncertain();
 e.pause().unwrap();e.toggle_play_pause(false).unwrap();e.set_volume(0).unwrap();
 assert_eq!(e.toggle_play_pause(false).unwrap_err(),"PLAYBACK_QUEUE_UNCERTAIN");
 server.join().unwrap();std::fs::remove_file(path).unwrap();std::fs::remove_dir(dir).unwrap();
}
