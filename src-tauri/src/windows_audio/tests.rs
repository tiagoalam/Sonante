use super::*;
use std::sync::atomic::{AtomicU64,Ordering};
static SERIAL:AtomicU64=AtomicU64::new(0);
fn direct()->Settings{Settings{audio_output_type:"alsa".into(),alsa_device:endpoint_key("{fixture-native-endpoint}"),..Settings::default()}}
#[test]fn explicit_stable_binding_and_confirmation_never_use_friendly_names_or_indexes(){
 let mut s=direct();assert_eq!(s.require_choice(),Err(direct_policy::CONFIRM));
 s.direct_volume_choice=Some(Choice{version:1,endpoint:s.alsa_device.clone(),dop_enabled:false});assert!(s.require_choice().is_ok());
 let text=s.output(Some("{fixture-native-endpoint}")).unwrap();assert!(text.contains("sonante_device_id \"{fixture-native-endpoint}\""));assert!(text.contains("sonante_direct_strict \"yes\""));assert!(text.contains("sonante_direct_pcm_negotiation \"restricted-v1\""));assert!(text.contains("dop \"yes\""));assert!(!text.contains("\n device "));
 assert!(s.output(Some("{another-endpoint}")).is_err());assert!(s.output(None).is_err());
 s.alsa_device=endpoint_key("{another-endpoint}");assert_eq!(s.require_choice(),Err(direct_policy::CONFIRM));
 for id in ["0","12name","Speakers","hw:0","hw:windows:invalid"]{s.alsa_device=id.into();assert!(s.validate().is_err());}
}
#[test]fn shared_default_output_preserves_approved_configuration_and_does_not_require_direct_choice(){
 let s=Settings::default();assert!(s.require_choice().is_ok());assert_eq!(s.output(None).unwrap(),"audio_output {\n type \"wasapi\"\n name \"Sonante Windows Shared\"\n exclusive \"no\"\n dop \"no\"\n mixer_type \"software\"\n}\n");
 let mut s=direct();s.audio_output_type="pipewire".into();assert!(!s.direct());assert_eq!(s.output(None).unwrap(),Settings::default().output(None).unwrap());
}
#[test]fn settings_reject_unimplemented_dop_and_invalid_buffers_or_processing_without_silent_fallback(){
 let mut s=direct();s.dop_enabled=true;assert!(s.validate().is_err());s.dop_enabled=false;
 for n in [0,1,512,4095,4097,u32::MAX]{s.audio_buffer_size_kb=n;assert!(s.validate().is_err());}
 s.audio_buffer_size_kb=4096;s.replay_gain="auto".into();assert!(s.validate().is_err());
 for mode in ["off","track","album"]{s.replay_gain=mode.into();assert!(s.validate().is_ok());}
}
#[test]fn corruption_preserves_bytes_and_never_resets_to_shared_or_default_volume(){
 let root=std::env::temp_dir().join(format!("sonante-windows-audio-own-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
 fs::create_dir(&root).unwrap();let path=root.join("audio.json");let store=Store::new(path.clone());
 assert_eq!(store.load().unwrap().settings,Settings::default());
 let mut saved=Saved::default();saved.settings=direct();saved.shared_volume=23;store.save(&saved).unwrap();let read=store.load().unwrap();assert_eq!(read.shared_volume,23);assert_eq!(read.settings,saved.settings);
 for bytes in [b"".as_slice(),b"{bad",b"{\"version\":1,\"version\":1}"]{fs::write(&path,bytes).unwrap();assert!(store.load().is_err());assert_eq!(fs::read(&path).unwrap(),bytes);}
 fs::remove_file(path).unwrap();fs::remove_file(root.join("audio.lock")).unwrap();fs::remove_dir(root).unwrap();
}
fn outputs()->Vec<String>{["outputid: 0","outputname: Sonante Windows Direct","plugin: wasapi","outputenabled: 1","attribute: sonante_endpoint_id={fixture-native-endpoint}","attribute: sonante_exclusive=yes","attribute: sonante_direct_strict=yes","attribute: sonante_direct_pcm_negotiation=restricted-v1","attribute: sonante_plugin_input=44100:16:2","attribute: sonante_plugin_selected=44100:16:2","attribute: sonante_pcm_conversion=none","attribute: sonante_render_rate=44100","attribute: sonante_render_channels=2","attribute: sonante_render_container_bits=16","attribute: sonante_render_valid_bits=16"].into_iter().map(str::to_owned).collect()}
fn with(lines:&[String],replacements:&[(&str,&str)])->Vec<String>{lines.iter().map(|line|replacements.iter().fold(line.clone(),|value,(old,new)|value.replace(old,new))).collect()}
#[test]fn direct_verification_accepts_exact_and_only_restricted_pcm_conversions(){
 let key=direct().alsa_device;let exact=outputs();let unchanged=negotiation::verify(&exact,&key,"44100:16:2").unwrap();assert!(!unchanged.converted);assert_eq!(unchanged.source.rate,44100);assert_eq!(unchanged.selected.rate,44100);
 for replacements in [
  vec![("sonante_plugin_selected=44100:16:2","sonante_plugin_selected=48000:16:2"),("sonante_pcm_conversion=none","sonante_pcm_conversion=converted"),("sonante_render_rate=44100","sonante_render_rate=48000")],
  vec![("sonante_plugin_selected=44100:16:2","sonante_plugin_selected=44100:24:2"),("sonante_pcm_conversion=none","sonante_pcm_conversion=converted"),("sonante_render_container_bits=16","sonante_render_container_bits=32"),("sonante_render_valid_bits=16","sonante_render_valid_bits=24")],
  vec![("sonante_plugin_input=44100:16:2","sonante_plugin_input=44100:24:2"),("sonante_plugin_selected=44100:16:2","sonante_plugin_selected=48000:16:2"),("sonante_pcm_conversion=none","sonante_pcm_conversion=converted"),("sonante_render_rate=44100","sonante_render_rate=48000")],
  vec![("sonante_plugin_selected=44100:16:2","sonante_plugin_selected=48000:24:2"),("sonante_pcm_conversion=none","sonante_pcm_conversion=converted"),("sonante_render_rate=44100","sonante_render_rate=48000"),("sonante_render_container_bits=16","sonante_render_container_bits=32"),("sonante_render_valid_bits=16","sonante_render_valid_bits=24")],
 ]{let converted=with(&exact,&replacements);let observed=negotiation::verify(&converted,&key,"44100:16:2").unwrap();assert!(observed.converted);}
 for (old,new,source) in [("sonante_plugin_selected=44100:16:2","sonante_plugin_selected=44100:16:1","44100:16:2"),("sonante_plugin_selected=44100:16:2","sonante_plugin_selected=44100:dsd:2","44100:16:2"),("sonante_plugin_input=44100:16:2","sonante_plugin_input=48000:16:2","44100:16:2"),("sonante_direct_pcm_negotiation=restricted-v1","sonante_direct_pcm_negotiation=no","44100:16:2")]{assert!(negotiation::verify(&with(&exact,&[(old,new)]),&key,source).is_err(),"{new}");}
 for (old,new) in [("outputid: 0","outputid: 1"),("plugin: wasapi","plugin: other"),("sonante_exclusive=yes","sonante_exclusive=no"),("sonante_direct_strict=yes","sonante_direct_strict=no"),("sonante_endpoint_id={fixture-native-endpoint}","sonante_endpoint_id=other"),("sonante_render_channels=2","sonante_render_channels=1"),("sonante_render_valid_bits=16","sonante_render_valid_bits=8")]{assert!(negotiation::verify(&with(&exact,&[(old,new)]),&key,"44100:16:2").is_err(),"{old}");}
 let mut duplicate=exact.clone();duplicate.push("outputid: 0".into());assert!(negotiation::verify(&duplicate,&key,"44100:16:2").is_err());
 let mut duplicate=exact.clone();duplicate.push("attribute: sonante_exclusive=yes".into());assert!(negotiation::verify(&duplicate,&key,"44100:16:2").is_err());
 assert!(negotiation::verify(&[],&key,"44100:16:2").is_err());
}

fn dop_outputs(container:u32)->Vec<String>{[
 "outputid: 0","outputname: Sonante Windows Direct","plugin: wasapi","outputenabled: 1",
 "attribute: sonante_endpoint_id={fixture-native-endpoint}","attribute: sonante_exclusive=yes",
 "attribute: sonante_direct_strict=yes","attribute: sonante_direct_pcm_negotiation=restricted-v1",
 "attribute: sonante_plugin_input=352800:dsd:2","attribute: sonante_plugin_selected=352800:dsd:2",
 "attribute: sonante_pcm_conversion=none","attribute: sonante_dsd_transport=dop",
 "attribute: sonante_dsd_conversion=none","attribute: sonante_render_rate=176400",
 "attribute: sonante_render_channels=2",
].into_iter().map(str::to_owned).chain([
 format!("attribute: sonante_render_container_bits={container}"),
 "attribute: sonante_render_valid_bits=24".to_owned(),
]).collect()}
#[test]fn strict_direct_dsd_requires_verified_dop_carrier_and_rejects_all_fallbacks(){
 let key=direct().alsa_device;
 for container in [24,32]{
  let observed=negotiation::verify_dop(&dop_outputs(container),&key,"352800:dsd:2").unwrap();
  assert_eq!(observed.source,negotiation::DsdFormat{rate:352800,channels:2});
  assert_eq!(observed.transport_rate,176400);
  assert_eq!(observed.transport_channels,2);
  assert_eq!(observed.render_container_bits,container);
  assert_eq!(observed.render_valid_bits,24);
 }
 let base=dop_outputs(24);
 for (old,new,source) in [
  ("sonante_exclusive=yes","sonante_exclusive=no","352800:dsd:2"),
  ("sonante_direct_strict=yes","sonante_direct_strict=no","352800:dsd:2"),
  ("sonante_endpoint_id={fixture-native-endpoint}","sonante_endpoint_id=other","352800:dsd:2"),
  ("sonante_dsd_transport=dop","sonante_dsd_transport=pcm","352800:dsd:2"),
  ("sonante_dsd_conversion=none","sonante_dsd_conversion=dsd_to_pcm","352800:dsd:2"),
  ("sonante_pcm_conversion=none","sonante_pcm_conversion=converted","352800:dsd:2"),
  ("sonante_plugin_input=352800:dsd:2","sonante_plugin_input=352800:dsd:1","352800:dsd:2"),
  ("sonante_plugin_selected=352800:dsd:2","sonante_plugin_selected=352800:16:2","352800:dsd:2"),
  ("sonante_render_rate=176400","sonante_render_rate=352800","352800:dsd:2"),
  ("sonante_render_channels=2","sonante_render_channels=1","352800:dsd:2"),
  ("sonante_render_valid_bits=24","sonante_render_valid_bits=16","352800:dsd:2"),
  ("sonante_render_container_bits=24","sonante_render_container_bits=16","352800:dsd:2"),
 ]{
  assert!(negotiation::verify_dop(&with(&base,&[(old,new)]),&key,source).is_err(),"{new}");
 }
 for source in ["352801:dsd:2","352800:16:2","352800:dsd:9"]{
  assert!(negotiation::verify_dop(&base,&key,source).is_err(),"{source}");
 }
}
