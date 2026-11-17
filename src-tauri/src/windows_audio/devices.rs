//! Enumerates active render endpoint identities without opening an audio client.
//! Availability is deliberately indeterminate until actual exclusive open.
use super::{endpoint_key,Result};
use serde::Serialize;
use std::{sync::atomic::{AtomicBool,Ordering},time::{Duration,Instant}};
use windows::{core::PWSTR,Win32::{Devices::FunctionDiscovery::PKEY_Device_FriendlyName,Media::Audio::{eRender,IMMDeviceEnumerator,MMDeviceEnumerator,DEVICE_STATE_ACTIVE},System::Com::{CoCreateInstance,CoInitializeEx,CoTaskMemFree,CoUninitialize,StructuredStorage::{PropVariantClear,PropVariantToStringAlloc,PROPVARIANT},CLSCTX_ALL,COINIT_MULTITHREADED,STGM_READ}}};
static BUSY:AtomicBool=AtomicBool::new(false);
struct Permit;impl Drop for Permit{fn drop(&mut self){BUSY.store(false,Ordering::Release);}}
struct Apartment;impl Drop for Apartment{fn drop(&mut self){unsafe{CoUninitialize()}}}
struct Allocated(PWSTR);impl Drop for Allocated{fn drop(&mut self){unsafe{CoTaskMemFree(Some(self.0.0.cast()))}}}
struct Variant(PROPVARIANT);impl Drop for Variant{fn drop(&mut self){if unsafe{PropVariantClear(&mut self.0)}.is_err(){eprintln!("Windows audio: property_cleanup_failed");}}}
#[derive(Serialize)]pub struct Device{pub id:String,pub name:String,#[serde(skip)]pub(crate) native_id:String}
fn text(p:PWSTR,limit:usize)->Result<String>{
 let p=Allocated(p);if p.0.is_null(){return Err("windows_audio_devices_unavailable");}let mut n=0;
 while n<=limit&&unsafe{*p.0.0.add(n)}!=0{n+=1;}
 if n==0||n>limit{return Err("windows_audio_devices_unavailable");}
 let s=String::from_utf16(unsafe{std::slice::from_raw_parts(p.0.0,n)}).map_err(|_|"windows_audio_devices_unavailable")?;
 if s.chars().any(char::is_control){return Err("windows_audio_devices_unavailable");}Ok(s)
}
pub fn list()->Result<Vec<Device>>{
 if BUSY.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("windows_audio_devices_busy");}let _permit=Permit;
 unsafe{CoInitializeEx(None,COINIT_MULTITHREADED)}.ok().map_err(|_|"windows_audio_devices_unavailable")?;let _com=Apartment;
 let end=Instant::now()+Duration::from_secs(2);
 let enumerator:IMMDeviceEnumerator=unsafe{CoCreateInstance(&MMDeviceEnumerator,None,CLSCTX_ALL)}.map_err(|_|"windows_audio_devices_unavailable")?;
 let collection=unsafe{enumerator.EnumAudioEndpoints(eRender,DEVICE_STATE_ACTIVE)}.map_err(|_|"windows_audio_devices_unavailable")?;
 let n=unsafe{collection.GetCount()}.map_err(|_|"windows_audio_devices_unavailable")?;if n>128{return Err("windows_audio_devices_limit");}
 let mut result=vec![Device{id:"default".into(),name:"Windows · WASAPI Shared".into(),native_id:String::new()}];
 for i in 0..n{
  if Instant::now()>=end{return Err("windows_audio_devices_timeout");}
  let device=unsafe{collection.Item(i)}.map_err(|_|"windows_audio_devices_unavailable")?;
  if unsafe{device.GetState()}.map_err(|_|"windows_audio_devices_unavailable")?!=DEVICE_STATE_ACTIVE{return Err("windows_audio_device_changed");}
  let native_id=text(unsafe{device.GetId()}.map_err(|_|"windows_audio_devices_unavailable")?,2048)?;
  let store=unsafe{device.OpenPropertyStore(STGM_READ)}.map_err(|_|"windows_audio_devices_unavailable")?;
  let variant=Variant(unsafe{store.GetValue(&PKEY_Device_FriendlyName)}.map_err(|_|"windows_audio_devices_unavailable")?);
  let name=text(unsafe{PropVariantToStringAlloc(&variant.0)}.map_err(|_|"windows_audio_devices_unavailable")?,512)?;
  let id=endpoint_key(&native_id);if result.iter().any(|d|d.id==id){return Err("windows_audio_device_changed");}
  result.push(Device{id,name,native_id});
 }
 Ok(result)
}
pub fn resolve(key:&str)->Result<String>{list()?.into_iter().find(|d|d.id==key&&d.id!="default").map(|d|d.native_id).ok_or("windows_audio_device_missing")}
