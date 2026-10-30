//! Owned MPD/process/PCM fixtures only. Never run against a personal session.
use super::*;
use crate::{config::AppConfig, config_save_operation::Coordinator, direct_playback::Gate, direct_volume::Session, supervisor::MpdSupervisor};
use std::{fs,io::Read,process::{Command,Stdio},sync::atomic::AtomicBool};
fn wait(mut check:impl FnMut()->bool){for _ in 0..300 {if check(){return}std::thread::sleep(Duration::from_millis(10));}panic!("owned fixture timeout")}
fn cfg(shared:bool,name:&str,muted:bool)->AppConfig{let mut c=AppConfig::default();c.first_run=false;c.audio_output_type=if shared{"pipewire"}else{"alsa"}.into();c.alsa_device=if shared{"default".into()}else{format!("hw:CARD={name},DEV=0")};c.direct_volume_choice=c.direct_context();c.direct_muted=muted;c}
#[test]
#[ignore="owned MPD namespace and generated paced file/null only"]
fn route_save_real_mpd_directions_states_duplicates_mute_and_release(){
 assert_eq!(std::env::var("SONANTE_DIRECT_GAIN_QA").unwrap(),"owned-file-null");
 let root=Path::new("/build/app-source/route-save-real-proof-verified");fs::create_dir(root).unwrap();
 let engine=Path::new("/usr/lib/sonante/runtimes/v1/mpd-direct");
 use sha2::{Digest,Sha256};assert_eq!(format!("{:x}",Sha256::digest(fs::read(engine).unwrap())),"7bf2a6c96fad672ee6a056335c1b8054a71107975c637b76f549751b43d0d4b1");
 for direction in ["shared-direct","direct-shared","direct-direct"]{for origin in ["playing","paused","stopped","muted"]{
  let dir=root.join(format!("{direction}-{origin}"));fs::create_dir(&dir).unwrap();let music=dir.join("music");fs::create_dir(&music).unwrap();
  let pcm=vec![0u8;44100*4*4];let mut wav=b"RIFF".to_vec();wav.extend_from_slice(&(36+pcm.len()as u32).to_le_bytes());wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");wav.extend_from_slice(&44100u32.to_le_bytes());wav.extend_from_slice(&176400u32.to_le_bytes());wav.extend_from_slice(b"\x04\0\x10\0data");wav.extend_from_slice(&(pcm.len()as u32).to_le_bytes());wav.extend_from_slice(&pcm);fs::write(music.join("fixture.wav"),wav).unwrap();
  let pipe=dir.join("capture.fifo");let c=std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();assert_eq!(unsafe{libc::mkfifo(c.as_ptr(),0o600)},0);
  let anchor=fs::OpenOptions::new().read(true).write(true).open(&pipe).unwrap();
  use std::os::unix::fs::OpenOptionsExt;let mut reader=fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&pipe).unwrap();
  let stop=Arc::new(AtomicBool::new(false));let flag=stop.clone();let drain=std::thread::spawn(move||{let mut b=[0u8;4096];while !flag.load(Ordering::Acquire){let _=reader.read(&mut b);std::thread::sleep(Duration::from_millis(2));}});
  let socket=dir.join("socket");let pidfile=dir.join("pid");let sock=socket.to_str().unwrap();let alsa=dir.join("asound.conf");fs::write(&alsa,format!("pcm.own_null {{ type null }}\npcm.own_output {{ type file slave.pcm \"own_null\" file \"{}\" format \"raw\" }}\n",pipe.display())).unwrap();
  let mut supervisor=MpdSupervisor::new_with_runtime_paths(&socket,&pidfile);let operations=Arc::new(Coordinator::new());let gate=Arc::new(Gate::new(operations.clone()));supervisor.attach_playback_gate(gate.clone());
  let spawn=|phase:&str,config:&AppConfig,sup:&mut MpdSupervisor|{
   let conf=dir.join(format!("{phase}.conf"));fs::write(&conf,format!("music_directory \"{}\"\ndb_file \"{}/db\"\nbind_to_address \"{}\"\npid_file \"{}\"\nlog_file \"{}/mpd.log\"\nauto_update \"no\"\nzeroconf_enabled \"no\"\nreplaygain \"off\"\ndecoder {{\n plugin \"wildmidi\"\n enabled \"no\"\n}}\naudio_output {{\n type \"alsa\"\n name \"owned paced file null\"\n device \"own_output\"\n pcm_eof_flush \"yes\"\n sonante_direct_strict \"yes\"\n mixer_type \"software\"\n replay_gain_handler \"none\"\n}}\n",music.display(),dir.display(),sock,pidfile.display(),dir.display())).unwrap();
   let log=fs::File::create(dir.join(format!("{phase}.log"))).unwrap();let child=Command::new(engine).arg("--no-daemon").arg(conf).env("ALSA_CONFIG_PATH",&alsa).stdin(Stdio::null()).stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();let pid=child.id();sup.set_process_for_test(child,true);
   let io=|cmd:&str|AudioEngine::send_owned_command_to_socket(sock,pid,cmd);wait(||io("status").is_ok());
   let session=if config.direct_context().is_some(){let s=Session::initialize(io).unwrap();if config.direct_muted{s.set_mute(true,false,&s.token(),io).unwrap();}Some(s)}else{io(if config.direct_muted{"setvol 0"}else{"setvol 80"}).unwrap();None};gate.bind(config,pid,session);pid
  };
  let muted=origin=="muted";let old=cfg(direction=="shared-direct","A",muted);let next=cfg(direction=="direct-shared","B",muted);let oldpid=spawn("old",&old,&mut supervisor);gate.commit_choice(&old);
  let io=|cmd:&str|AudioEngine::send_command_to_socket(sock,cmd);io("update").unwrap();wait(||!io("status").unwrap().iter().any(|s|s.starts_with("updating_db:")));
  let mut audio=AudioEngine::new(sock,music.to_str().unwrap());audio.attach_direct_playback(gate.clone());
  let t=TrackMetadata{title:"Fixture".into(),artist:"QA".into(),album:"QA".into(),thumb:None,plex_image:None,media_locator:None,uri:"fixture.wav".into(),duration:Some(4.)};audio.play_tracks(vec![t.clone(),t.clone(),t],vec!["fixture.wav".into();3],1).unwrap();
  if origin=="paused"{audio.pause().unwrap();}if origin=="stopped"{io("stop").unwrap();}
  let owner=operations.begin().unwrap();let resolved=audio.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into();3]).unwrap();let watch=audio.watch_eq_transition();let saved=audio.prepare_reserved_route_save(&resolved).unwrap();
  supervisor.stop().unwrap();assert!(!Path::new(&format!("/proc/{oldpid}")).exists());let newpid=spawn("new",&next,&mut supervisor);
  assert_eq!(audio.restore_for_route_save(saved,&resolved).unwrap(),DeviceSwitchRestoreOutcome::Complete);let before=io("status").unwrap();assert!(before.contains(&"state: stop".into()));gate.commit_choice(&next);
  let result=audio.resume_saved_route(saved,&resolved,&watch,&owner,|audio|{if muted{assert_eq!(audio.observed_software_volume().unwrap(),0);}Ok(())}).unwrap();assert_eq!(result,if origin=="paused"{"paused"}else if origin=="stopped"{"stopped"}else{"resumed"});
  let after=io("status").unwrap();if origin=="playing"||muted{assert!(after.contains(&"state: play".into()));assert!(after.contains(&"song: 1".into()));assert!(after.contains(&format!("volume: {}",if muted{0}else if next.direct_context().is_some(){100}else{80})));}
  assert_eq!(io("playlistinfo").unwrap().iter().filter(|s|s.starts_with("file:")).count(),3);
  supervisor.stop().unwrap();assert!(!Path::new(&format!("/proc/{newpid}")).exists());stop.store(true,Ordering::Release);drain.join().unwrap();drop(anchor);
  println!("[RouteSaveQA] direction={direction} origin={origin} actual_AudioEngine_Gate_Session=true duplicate_index_1=true old_new_childs_reaped=true");
 }}
}
