// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
//! Windows PCM transport ABI/core. No DSP, device, filesystem or engine access.
use std::{ptr::NonNull, sync::atomic::{AtomicU32, AtomicU64, Ordering}};
pub const MAGIC:u64=0x31304d43504e4f53;
pub const CAPACITY:u64=4096;
pub const FRAME_BYTES:usize=8;
pub const EOF:u32=1;pub const LOST:u32=2;pub const STOP:u32=4;
const INDEX_MAX:u64=0x7fffffffffffffff;
#[repr(C,align(64))]
pub struct Header {
 pub magic:u64,pub version:u32,pub header_bytes:u32,pub capacity:u32,pub rate:u32,pub channels:u32,pub frame_bytes:u32,pub format:u32,pub owner:u32,
 pub nonce0:u64,pub nonce1:u64,pub reserved:u64,
 pub written:u64,pub pad_written:[u8;56],pub read:u64,pub pad_read:[u8;56],
 pub flags:u32,pub consumer:u32,pub pad_flags:[u8;56],pub data:[u8;32768],
}
const _:()=assert!(size_of::<Header>()==33024 && align_of::<Header>()==64);
const _:()=assert!(std::mem::offset_of!(Header,written)==64 && std::mem::offset_of!(Header,read)==128);
const _:()=assert!(std::mem::offset_of!(Header,flags)==192 && std::mem::offset_of!(Header,data)==256);
const _:()=assert!(std::mem::offset_of!(Header,consumer)==196);
#[derive(Clone,Copy)]
pub struct Binding {pub owner:u32,pub nonce0:u64,pub nonce1:u64}
#[derive(Debug,PartialEq)]
pub enum Fault {Binding,State,Alignment}
pub struct Reader {p:NonNull<Header>,binding:Binding}
// Mapping lifetime is owned by the native wrapper; one consumer only. This is
// Send so Camilla's capture thread can own it, never Sync or clonable.
unsafe impl Send for Reader {}
unsafe fn u64_load(p:*mut u64)->u64 {unsafe{AtomicU64::from_ptr(p)}.load(Ordering::Acquire)}
unsafe fn u32_load(p:*mut u32)->u32 {unsafe{AtomicU32::from_ptr(p)}.load(Ordering::Acquire)}
impl Reader {
 /// Native capture adapter claims the sole consumer only after binding/format
 /// validation. The flag is never cleared; a generation cannot be resurrected.
 pub fn claim(&self)->Result<(),Fault>{
  self.validate()?;
  if self.flags()?!=0{return Err(Fault::State);}
  unsafe{AtomicU32::from_ptr(std::ptr::addr_of_mut!((*self.p.as_ptr()).consumer))}.compare_exchange(0,1,Ordering::AcqRel,Ordering::Acquire).map(|_|()).map_err(|_|Fault::State)?;
  if self.flags()?!=0{return Err(Fault::State);}Ok(())
 }
 pub fn invalidate(&self){unsafe{AtomicU32::from_ptr(std::ptr::addr_of_mut!((*self.p.as_ptr()).flags))}.fetch_or(LOST|STOP,Ordering::AcqRel);}
 pub fn flags(&self)->Result<u32,Fault>{self.validate()?;Ok(unsafe{u32_load(std::ptr::addr_of_mut!((*self.p.as_ptr()).flags))})}
 /// Caller owns a live, exact-size, 64-aligned mapping until this view is dropped.
 /// Only one reader may exist; immutable ABI metadata is initialized before use.
 pub unsafe fn new(p:*mut Header,binding:Binding)->Result<Self,Fault>{
  let p=NonNull::new(p).filter(|p|p.as_ptr() as usize%64==0).ok_or(Fault::Binding)?;
  let this=Self{p,binding};this.validate()?;Ok(this)
 }
 pub fn validate(&self)->Result<(),Fault>{
  let p=self.p.as_ptr();let b=self.binding;
  let ok=unsafe{
   b.owner!=0&&(b.nonce0|b.nonce1)!=0&&
   u64_load(std::ptr::addr_of_mut!((*p).magic))==MAGIC&&
   u32_load(std::ptr::addr_of_mut!((*p).version))==1&&u32_load(std::ptr::addr_of_mut!((*p).header_bytes))==256&&
   u32_load(std::ptr::addr_of_mut!((*p).capacity))==CAPACITY as u32&&u32_load(std::ptr::addr_of_mut!((*p).rate))==48000&&
   u32_load(std::ptr::addr_of_mut!((*p).channels))==2&&u32_load(std::ptr::addr_of_mut!((*p).frame_bytes))==8&&
   u32_load(std::ptr::addr_of_mut!((*p).format))==1&&u32_load(std::ptr::addr_of_mut!((*p).owner))==b.owner&&
   u64_load(std::ptr::addr_of_mut!((*p).nonce0))==b.nonce0&&u64_load(std::ptr::addr_of_mut!((*p).nonce1))==b.nonce1&&
   u64_load(std::ptr::addr_of_mut!((*p).reserved))==0&&u32_load(std::ptr::addr_of_mut!((*p).consumer))<=1&&u32_load(std::ptr::addr_of_mut!((*p).flags))&!(EOF|LOST|STOP)==0
  };if ok {Ok(())}else{Err(Fault::Binding)}
 }
 /// None is empty/backpressure, Some(0) EOF/explicit stop, never fabricated data.
 pub fn read(&mut self,out:&mut[u8])->Result<Option<usize>,Fault>{
  self.validate()?;if out.len()%FRAME_BYTES!=0{return Err(Fault::Alignment);}
  if out.is_empty(){return Ok(Some(0));}
  let p=self.p.as_ptr();let(flags,read,written)=unsafe{(u32_load(std::ptr::addr_of_mut!((*p).flags)),u64_load(std::ptr::addr_of_mut!((*p).read)),u64_load(std::ptr::addr_of_mut!((*p).written)))};
  if flags&LOST!=0{return Err(Fault::State);}if flags&STOP!=0{return Ok(Some(0));}
  if read>written||written>INDEX_MAX||written-read>CAPACITY{return Err(Fault::State);}
  let frames=(out.len()/FRAME_BYTES) as u64;let count=frames.min(written-read);
  if count==0{return Ok(if flags&EOF!=0{Some(0)}else{None});}
  let bytes=count as usize*FRAME_BYTES;let offset=(read%CAPACITY)as usize*FRAME_BYTES;let first=bytes.min(32768-offset);
  unsafe{
   let data=std::ptr::addr_of!((*p).data).cast::<u8>();
   std::ptr::copy_nonoverlapping(data.add(offset),out.as_mut_ptr(),first);
   std::ptr::copy_nonoverlapping(data,out.as_mut_ptr().add(first),bytes-first);
   let flags=u32_load(std::ptr::addr_of_mut!((*p).flags));
   if flags&(LOST|STOP)!=0{return Err(Fault::State);}
   AtomicU64::from_ptr(std::ptr::addr_of_mut!((*p).read)).store(read+count,Ordering::Release);
  }
  Ok(Some(bytes))
 }
}
