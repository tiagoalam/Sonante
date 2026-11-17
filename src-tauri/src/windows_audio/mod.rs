//! Windows route settings. The original UI uses its established configuration
//! schema; native endpoint IDs never become enumeration indexes or names.
use crate::windows_mpd::protocol::{self,Result};
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use std::{fs,io::Read,path::{Path,PathBuf},sync::Arc};
#[cfg(windows)]
pub mod devices;
pub(crate) mod direct_policy;
pub(crate) mod negotiation;
pub(crate) mod admission;
pub(crate) fn policy_error(error:String)->&'static str{match error.as_str(){direct_policy::CONFIRM=>direct_policy::CONFIRM,direct_policy::GRADUAL=>direct_policy::GRADUAL,_=>direct_policy::STALE}}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct Choice {pub version:u8,pub endpoint:String,pub dop_enabled:bool}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
 pub audio_output_type:String,pub alsa_device:String,pub dop_enabled:bool,
 pub audio_buffer_size_kb:u32,pub replay_gain:String,
 #[serde(default)]pub direct_volume_choice:Option<Choice>,
 #[serde(default)]pub direct_muted:bool,
}
impl Default for Settings {fn default()->Self{Self{audio_output_type:"pipewire".into(),alsa_device:"default".into(),dop_enabled:false,audio_buffer_size_kb:4096,replay_gain:"off".into(),direct_volume_choice:None,direct_muted:false}}}
pub fn endpoint_key(id:&str)->String{format!("hw:windows:{:x}",Sha256::digest(id.as_bytes()))}
fn valid_key(id:&str)->bool{id.strip_prefix("hw:windows:").is_some_and(|s|s.len()==64&&s.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c)))}
impl Settings {
 pub fn direct(&self)->bool{matches!(self.audio_output_type.as_str(),"alsa"|"pipewire")&&crate::audio_route::interpret_legacy_route(&self.audio_output_type,&self.alsa_device).route.access==crate::audio_route::AudioAccess::Exclusive}
 pub fn validate(&self)->Result<()>{
  if !matches!(self.audio_output_type.as_str(),"alsa"|"pipewire")||self.alsa_device!="default"&&!valid_key(&self.alsa_device)||self.dop_enabled||!matches!(self.audio_buffer_size_kb,4096|8192|16384|32768)||!matches!(self.replay_gain.as_str(),"off"|"track"|"album"){return Err("windows_audio_invalid");}
  if let Some(c)=&self.direct_volume_choice {if c.version!=1||!valid_key(&c.endpoint)||c.dop_enabled{return Err("windows_audio_invalid");}}
  Ok(())
 }
 pub fn confirmed(&self)->bool{!self.direct()||self.direct_volume_choice.as_ref()==Some(&Choice{version:1,endpoint:self.alsa_device.clone(),dop_enabled:self.dop_enabled})}
 pub fn require_choice(&self)->Result<()>{if self.confirmed(){Ok(())}else{Err(direct_policy::CONFIRM)}}
 pub fn output(&self,endpoint:Option<&str>)->Result<String>{
  self.validate()?;
  if !self.direct(){return Ok("audio_output {\n type \"wasapi\"\n name \"Sonante Windows Shared\"\n exclusive \"no\"\n dop \"no\"\n mixer_type \"software\"\n}\n".into());}
  let endpoint=endpoint.ok_or("windows_audio_device_missing")?;
  if endpoint.is_empty()||endpoint.len()>2048||endpoint.chars().any(char::is_control)||endpoint_key(endpoint)!=self.alsa_device{return Err("windows_audio_device_changed");}
  Ok(format!("audio_output {{\n type \"wasapi\"\n name \"Sonante Windows Direct\"\n exclusive \"yes\"\n sonante_direct_strict \"yes\"\n sonante_direct_pcm_negotiation \"restricted-v1\"\n sonante_device_id {}\n dop \"yes\"\n mixer_type \"software\"\n}}\n",protocol::quote(endpoint)?))
 }
 pub fn same_route(&self,other:&Self)->bool{self.audio_output_type==other.audio_output_type&&self.alsa_device==other.alsa_device&&self.dop_enabled==other.dop_enabled&&self.audio_buffer_size_kb==other.audio_buffer_size_kb&&self.replay_gain==other.replay_gain}
}
#[derive(Clone,Serialize,Deserialize,PartialEq,Eq)]#[serde(deny_unknown_fields)]
pub struct Saved {version:u8,pub settings:Settings,pub shared_volume:u8}
impl Default for Saved{fn default()->Self{Self{version:1,settings:Settings::default(),shared_volume:50}}}
#[derive(Clone)]pub struct Store(Arc<PathBuf>);
impl Store {
 pub fn new(path:PathBuf)->Self{Self(Arc::new(path))}
 pub fn load(&self)->Result<Saved>{
  crate::persistence::prepare_private_file_for_load(&self.0,"Windows audio").map_err(|_|"windows_audio_storage")?;
  let f=match fs::File::open(&*self.0){Ok(f)=>f,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(Saved::default()),Err(_)=>return Err("windows_audio_storage")};
  let mut bytes=Vec::new();f.take(16385).read_to_end(&mut bytes).map_err(|_|"windows_audio_storage")?;if bytes.len()>16384{return Err("windows_audio_storage");}
  let saved:Saved=serde_json::from_slice(&bytes).map_err(|_|"windows_audio_storage")?;
  if saved.version!=1||saved.shared_volume>100{return Err("windows_audio_storage");}saved.settings.validate().map_err(|_|"windows_audio_storage")?;Ok(saved)
 }
 pub fn save(&self,value:&Saved)->Result<()>{
  value.settings.validate()?;if value.version!=1||value.shared_volume>100{return Err("windows_audio_invalid");}
  let _lock=crate::persistence::operation_lock(&self.0.with_extension("lock")).map_err(|_|"windows_audio_busy")?;
  let bytes=serde_json::to_vec(value).map_err(|_|"windows_audio_storage")?;
  crate::persistence::atomic_write_private(&self.0,&bytes,"Windows audio").map_err(|_|"windows_audio_storage")
 }
 pub fn path(&self)->&Path{&self.0}
}
// Only the fields consumed by the original Direct policy. No hardware writes
// or alternate audio engine live in this projection adapter.
pub(crate) mod projection {
 #[derive(Clone,Copy,PartialEq,Eq)]pub enum VolumeBackend{DirectEquipment,Unavailable}
 pub struct Volume{pub backend:VolumeBackend,pub writable:bool,pub direct_token:Option<String>,pub direct_gain:Option<super::direct_policy::Gain>,pub muted:bool,pub value:u8,pub available:bool}
 pub struct Processing{pub mpd_volume:Option<u8>}
 pub struct PlaybackStatus{pub volume:Volume,pub processing:Option<Processing>,pub snapshot_consistent:bool}
}
#[derive(Clone,Serialize)]#[serde(rename_all="camelCase")]
pub struct DirectSnapshot{pub token:Option<String>,pub gain:Option<direct_policy::Gain>,pub muted:bool,pub available:bool,pub consistent:bool}
pub(crate) fn direct_snapshot(policy:&Arc<direct_policy::Session>,volume:u8)->DirectSnapshot{
 use projection::*;
 let before=direct_policy::Context::new(VolumeBackend::DirectEquipment,Some(policy.clone()));
 let mut status=PlaybackStatus{volume:Volume{backend:VolumeBackend::DirectEquipment,writable:false,direct_token:None,direct_gain:None,muted:false,value:volume,available:true},processing:Some(Processing{mpd_volume:Some(volume)}),snapshot_consistent:true};
 before.apply(&direct_policy::Context::new(VolumeBackend::DirectEquipment,Some(policy.clone())),&mut status);
 DirectSnapshot{token:status.volume.direct_token,gain:status.volume.direct_gain,muted:status.volume.muted,available:status.volume.available,consistent:status.snapshot_consistent}
}
#[cfg(test)]mod tests;
