use super::*;
use crate::{config::AppConfig,config_save_operation::Coordinator,direct_volume::Session,direct_playback::Gate};
use std::{fs,process::{Child,Command,Stdio},time::{Instant,Duration},thread};
struct Own(Child);impl Drop for Own{fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}
fn wait(mut f:impl FnMut()->bool){let end=Instant::now()+Duration::from_secs(8);while !f(){assert!(Instant::now()<end);thread::sleep(Duration::from_millis(10));}}
#[test]
#[ignore="own MPD file/null namespace only"]
fn real_audioengine_normal_direct_play_and_persisted_mute(){
 assert_eq!(std::env::var("SONANTE_DIRECT_GAIN_QA").unwrap(),"owned-file-null");
 let engine=Path::new("/usr/lib/sonante/runtimes/v1/mpd-direct");
 use sha2::{Digest,Sha256};assert_eq!(format!("{:x}",Sha256::digest(fs::read(engine).unwrap())),"7bf2a6c96fad672ee6a056335c1b8054a71107975c637b76f549751b43d0d4b1");
 let root=Path::new("/build/app-source/normal-real-proof");fs::create_dir(root).unwrap();
 for muted in[false,true]{
  let dir=root.join(if muted{"intentional-mute"}else{"normal-play"});fs::create_dir(&dir).unwrap();let music=dir.join("music");fs::create_dir(&music).unwrap();let capture=dir.join("capture.raw");let socket=dir.join("socket");let socket=socket.to_str().unwrap();
  let mut pcm=vec![];for i in 0..4417i32{pcm.extend_from_slice(&(((i*7919)%60001-30000)as i16).to_le_bytes());pcm.extend_from_slice(&(((i*104729)%50001-25000)as i16).to_le_bytes());}
  let mut wav=b"RIFF".to_vec();wav.extend_from_slice(&(36+pcm.len()as u32).to_le_bytes());wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");wav.extend_from_slice(&44100u32.to_le_bytes());wav.extend_from_slice(&176400u32.to_le_bytes());wav.extend_from_slice(b"\x04\0\x10\0data");wav.extend_from_slice(&(pcm.len()as u32).to_le_bytes());wav.extend_from_slice(&pcm);fs::write(music.join("fixture.wav"),wav).unwrap();
  let asound=dir.join("asound.conf");fs::write(&asound,format!("pcm.own_null {{ type null }}\npcm.own_output {{ type file slave.pcm \"own_null\" file \"{}\" format \"raw\" }}\n",capture.display())).unwrap();
  let conf=dir.join("mpd.conf");fs::write(&conf,format!("music_directory \"{}\"\ndb_file \"{}/db\"\nbind_to_address \"{}\"\nlog_file \"{}/mpd.log\"\nauto_update \"no\"\nzeroconf_enabled \"no\"\nreplaygain \"off\"\nvolume_normalization \"no\"\ndecoder {{\n plugin \"wildmidi\"\n enabled \"no\"\n}}\naudio_output {{\n type \"alsa\"\n name \"own file null\"\n device \"own_output\"\n pcm_eof_flush \"yes\"\n sonante_direct_strict \"yes\"\n mixer_type \"software\"\n replay_gain_handler \"none\"\n}}\n",music.display(),dir.display(),socket,dir.display())).unwrap();
  let log=fs::File::create(dir.join("process.log")).unwrap();let mut child=Own(Command::new(engine).arg("--no-daemon").arg(&conf).env("ALSA_CONFIG_PATH",&asound).stdin(Stdio::null()).stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap());let pid=child.0.id();
  let command=|c:&str|AudioEngine::send_owned_command_to_socket(socket,pid,c);wait(||command("status").is_ok());let session=Session::initialize(command).unwrap();command("update").unwrap();wait(||!command("status").unwrap().iter().any(|s|s.starts_with("updating_db:")));
  let mut cfg=AppConfig::default();cfg.first_run=false;cfg.audio_output_type="alsa".into();cfg.alsa_device="hw:CARD=OwnFixture,DEV=0".into();cfg.direct_muted=muted;
  if muted{session.set_mute(true,false,&session.token(),command).unwrap();}
  let gate=Arc::new(Gate::new(Arc::new(Coordinator::new())));gate.bind(&cfg,pid,Some(session));
  let mut audio=AudioEngine::new(socket,music.to_str().unwrap());audio.attach_direct_playback(gate.clone());
  let track=TrackMetadata{title:"Fixture".into(),artist:"QA".into(),album:"QA".into(),thumb:None,plex_image:None,media_locator:None,uri:"fixture.wav".into(),duration:Some(0.1)};
  let previous=audio.get_queue();
  assert!(audio.play_tracks(vec![track.clone()],vec!["fixture.wav".into()],0).unwrap_err().contains(crate::direct_volume::CONFIRM));assert!(audio.queue_coherent);assert_eq!(audio.get_queue().iter().map(|t|&t.uri).collect::<Vec<_>>(),previous.iter().map(|t|&t.uri).collect::<Vec<_>>());
  cfg.direct_volume_choice=cfg.direct_context();gate.commit_choice(&cfg);
  audio.play_tracks(vec![track],vec!["fixture.wav".into()],0).unwrap();wait(||{let status=command("status").unwrap();assert!(!status.iter().any(|s|s.starts_with("error:")),"{status:?}");status.iter().any(|s|s=="state: stop")});
  let status=command("status").unwrap();assert!(status.contains(&format!("volume: {}",if muted{0}else{100})));command("disableoutput 0").unwrap();child.0.kill().unwrap();child.0.wait().unwrap();
  let raw=fs::read(capture).unwrap();assert!(raw.len()>=pcm.len()*2);for (i,word) in raw[..pcm.len()*2].chunks_exact(4).enumerate(){let sample=i32::from_le_bytes(word.try_into().unwrap());assert_eq!(sample&255,0);let expected=if muted{0}else{i16::from_le_bytes(pcm[i*2..i*2+2].try_into().unwrap())as i32};assert_eq!(sample>>8,expected);}
  println!("[DirectNormalQA] case={} guard=real_AudioEngine pid_bound=true initial_zero=true requested_play_only=true pcm_samples_verified=true",if muted{"intentional-mute"}else{"normal-play"});
 }
}
