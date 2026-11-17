//! Anonymous SPSC PCM mapping. Original handles are never inheritable; each
//! child receives four short-lived duplicates through the exact startup list.
use super::pcm_core as core;
use crate::windows_mpd::protocol::Result;
use std::{ffi::c_void, mem::{size_of, zeroed}, os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle}, ptr::{null, null_mut, NonNull}, sync::{mpsc, Arc, atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering}}, thread::{self, JoinHandle}, time::Duration};
use windows_sys::Win32::{Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0}, Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser, SECURITY_ATTRIBUTES}, Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW}, System::{Memory::{CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE, MEMORY_MAPPED_VIEW_ADDRESS}, Threading::{CreateEventW, GetCurrentProcess, GetCurrentProcessId, OpenProcessToken, SetEvent, WaitForMultipleObjects, WaitForSingleObject}}, System::JobObjects::TerminateJobObject};

fn owned(raw: HANDLE) -> Result<OwnedHandle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE { return Err("pcm_handle_failed"); }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}
fn duplicate(raw: HANDLE, inherit: bool) -> Result<OwnedHandle> {
    let mut out = null_mut();
    let current = unsafe { GetCurrentProcess() };
    if unsafe { DuplicateHandle(current, raw, current, &mut out, 0, inherit as i32, DUPLICATE_SAME_ACCESS) } == 0 { return Err("pcm_handle_failed"); }
    owned(out)
}
struct Local(*mut c_void);
impl Drop for Local { fn drop(&mut self) { unsafe { LocalFree(self.0); } } }

// Explicit current-user + SYSTEM DACL. No names, Everyone, default DACL or
// permissive filesystem endpoint is used for audio memory/events.
pub struct PrivateSecurity(Local);
impl PrivateSecurity {
    pub fn new() -> Result<Self> {
        Self::create(false)
    }
    pub fn directory() -> Result<Self> {
        Self::create(true)
    }
    fn create(directory:bool) -> Result<Self> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 { return Err("pcm_security_failed"); }
        let token = owned(token)?;
        let mut bytes = 0;
        unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut bytes); }
        if !(size_of::<TOKEN_USER>() as u32..=65536).contains(&bytes) { return Err("pcm_security_failed"); }
        let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, storage.as_mut_ptr().cast(), bytes, &mut bytes) } == 0 { return Err("pcm_security_failed"); }
        let sid = unsafe { (*(storage.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut sid_text = null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut sid_text) } == 0 { return Err("pcm_security_failed"); }
        let allocation = Local(sid_text.cast());
        let mut len = 0;
        while len < 256 && unsafe { *sid_text.add(len) } != 0 { len += 1; }
        if len == 256 { return Err("pcm_security_failed"); }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, len) }).map_err(|_| "pcm_security_failed")?;
        drop(allocation);
        let inherit=if directory {"OICI"}else{""};
        let sddl: Vec<u16> = format!("D:P(A;{inherit};GA;;;SY)(A;{inherit};GA;;;{sid})").encode_utf16().chain([0]).collect();
        let mut descriptor = null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, null_mut()) } == 0 { return Err("pcm_security_failed"); }
        Ok(Self(Local(descriptor)))
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.0.0, bInheritHandle: 0 }
    }
}

pub struct ChildHandles { handles: [OwnedHandle; 4], pub owner: u32, pub nonce0: u64, pub nonce1: u64 }
impl ChildHandles {
    pub fn raw(&self) -> [HANDLE; 4] { std::array::from_fn(|i| self.handles[i].as_raw_handle()) }
    pub fn parameters(&self) -> serde_json::Value {
        let h = self.raw();
        serde_json::json!({"mapping":(h[0] as usize).to_string(),"readable":(h[1] as usize).to_string(),"writable":(h[2] as usize).to_string(),"stop":(h[3] as usize).to_string(),"owner_pid":self.owner,"nonce0":self.nonce0.to_string(),"nonce1":self.nonce1.to_string()})
    }
    pub fn mpd_config(&self) -> String {
        let h = self.raw();
        format!("sonante_mapping_handle \"{}\"\nsonante_readable_handle \"{}\"\nsonante_writable_handle \"{}\"\nsonante_stop_handle \"{}\"\nsonante_owner_pid \"{}\"\nsonante_nonce0 \"{}\"\nsonante_nonce1 \"{}\"\n",h[0] as usize,h[1] as usize,h[2] as usize,h[3] as usize,self.owner,self.nonce0,self.nonce1)
    }
}
pub struct Ring {
    p: NonNull<core::Header>, handles: [OwnedHandle; 4], binding: core::Binding,
}
// Metadata initialized before sharing; parent only touches interlocked fields.
// Mapping/handles remain alive until final Arc drops, after the guardian joins.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}
#[derive(Debug, serde::Serialize)]
pub struct Snapshot { pub read: u64, pub written: u64, pub flags: u32, pub consumer_ready:bool }
impl Ring {
    /// Immutable binding only: no duplicate handles or additional PCM reader.
    pub(crate) fn observation_key(&self)->(u32,u64,u64){(self.binding.owner,self.binding.nonce0,self.binding.nonce1)}
    pub fn new() -> Result<Arc<Self>> {
        let security = PrivateSecurity::new()?; let sa = security.attributes();
        let mapping = owned(unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, &sa, PAGE_READWRITE, 0, size_of::<core::Header>() as u32, null()) })?;
        let readable = owned(unsafe { CreateEventW(&sa, 0, 0, null()) })?;
        let writable = owned(unsafe { CreateEventW(&sa, 0, 0, null()) })?;
        let stop = owned(unsafe { CreateEventW(&sa, 1, 0, null()) })?;
        let view = unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_ALL_ACCESS, 0, 0, size_of::<core::Header>()) };
        let p = match NonNull::new(view.Value.cast::<core::Header>()).filter(|p| p.as_ptr() as usize % 64 == 0) {
            Some(p)=>p,
            None=>{if !view.Value.is_null(){unsafe{UnmapViewOfFile(view);}}return Err("pcm_mapping_failed");},
        };
        // Establish Rust/C++ object layout before any handle is duplicated.
        unsafe { p.as_ptr().write(zeroed()); }
        let mut nonce = [0u8; 16];
        if getrandom::fill(&mut nonce).is_err() { unsafe { UnmapViewOfFile(view); } return Err("pcm_nonce_failed"); }
        let binding = core::Binding { owner: unsafe { GetCurrentProcessId() }, nonce0: u64::from_le_bytes(nonce[..8].try_into().unwrap()), nonce1: u64::from_le_bytes(nonce[8..].try_into().unwrap()) };
        if binding.nonce0 | binding.nonce1 == 0 { unsafe { UnmapViewOfFile(view); } return Err("pcm_nonce_failed"); }
        unsafe {
            let h = &mut *p.as_ptr(); h.magic=core::MAGIC; h.version=1; h.header_bytes=256; h.capacity=core::CAPACITY as u32; h.rate=48000; h.channels=2; h.frame_bytes=8; h.format=1; h.owner=binding.owner; h.nonce0=binding.nonce0; h.nonce1=binding.nonce1;
        }
        Ok(Arc::new(Self { p, handles:[mapping,readable,writable,stop], binding }))
    }
    pub fn child(&self) -> Result<ChildHandles> {
        // If an intermediate duplicate fails, already-created handles close.
        let handles = [duplicate(self.handles[0].as_raw_handle(),true)?,duplicate(self.handles[1].as_raw_handle(),true)?,duplicate(self.handles[2].as_raw_handle(),true)?,duplicate(self.handles[3].as_raw_handle(),true)?];
        Ok(ChildHandles { handles, owner:self.binding.owner, nonce0:self.binding.nonce0, nonce1:self.binding.nonce1 })
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let p = self.p.as_ptr();
        for _ in 0..4 {
            let (first,written,read,flags) = unsafe { (AtomicU64::from_ptr(std::ptr::addr_of_mut!((*p).read)).load(Ordering::Acquire),AtomicU64::from_ptr(std::ptr::addr_of_mut!((*p).written)).load(Ordering::Acquire),AtomicU64::from_ptr(std::ptr::addr_of_mut!((*p).read)).load(Ordering::Acquire),AtomicU32::from_ptr(std::ptr::addr_of_mut!((*p).flags)).load(Ordering::Acquire)) };
            if first != read { continue; }
            if read>written || written>i64::MAX as u64 || written-read>core::CAPACITY || flags&!(core::EOF|core::LOST|core::STOP)!=0 { return Err("pcm_state_invalid"); }
            let consumer=unsafe{AtomicU32::from_ptr(std::ptr::addr_of_mut!((*p).consumer)).load(Ordering::Acquire)};
            if consumer>1{return Err("pcm_state_invalid");}
            return Ok(Snapshot { read,written,flags,consumer_ready:consumer==1 });
        }
        Err("pcm_observation_busy")
    }
    fn flag(&self, value:u32) -> Result<()> {
        unsafe { AtomicU32::from_ptr(std::ptr::addr_of_mut!((*self.p.as_ptr()).flags)).fetch_or(value,Ordering::AcqRel); }
        let mut good=true;
        for h in &self.handles[1..3] { good &= unsafe { SetEvent(h.as_raw_handle()) } != 0; }
        if good { Ok(()) } else { Err("pcm_signal_failed") }
    }
    pub fn eof(&self) -> Result<()> { self.flag(core::EOF) }
    pub fn stop(&self, lost:bool) -> Result<()> {
        let result=self.flag(core::STOP | if lost { core::LOST } else {0});
        let signalled=unsafe { SetEvent(self.handles[3].as_raw_handle()) } != 0;
        result?;
        if signalled { Ok(()) } else { Err("pcm_signal_failed") }
    }
}
impl Drop for Ring { fn drop(&mut self) { if self.stop(true).is_err() { eprintln!("PCM: signal_failed"); } unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value:self.p.as_ptr().cast() }); } } }

pub struct Witness { process: OwnedHandle, job: OwnedHandle }
impl Witness {
    pub fn duplicate(process:HANDLE,job:HANDLE) -> Result<Self> { Ok(Self { process:duplicate(process,false)?, job:duplicate(job,false)? }) }
    fn terminate(&self) -> bool {
        // The duplicated Job is private, exact, and not inherited by a child.
        let signalled = unsafe { TerminateJobObject(self.job.as_raw_handle(),1) } != 0;
        let waited = crate::windows_mpd::job_scope::owned(self.process.as_raw_handle(),self.job.as_raw_handle(),Duration::from_secs(2)).is_ok();
        signalled && waited
    }
}
static WATCHERS:AtomicUsize=AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn new()->Result<Self> { WATCHERS.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<2).then_some(n+1)).map(|_|Self).map_err(|_|"pcm_busy") }
}
impl Drop for Permit {fn drop(&mut self){WATCHERS.fetch_sub(1,Ordering::AcqRel);}}
pub struct Guardian { cancel: OwnedHandle, done:mpsc::Receiver<Result<()>>, worker: Option<JoinHandle<()>> }
impl Guardian {
    pub fn start(ring:Arc<Ring>, producer:Witness, consumer:Witness) -> Result<Self> {
        Self::start_guarded(ring,producer,consumer,None)
    }
    pub fn start_guarded(ring:Arc<Ring>, producer:Witness, consumer:Witness, probe:Option<Arc<dyn Fn()->bool+Send+Sync>>) -> Result<Self> {
        let cancel = owned(unsafe { CreateEventW(null(),1,0,null()) })?;
        let wait_cancel = duplicate(cancel.as_raw_handle(),false)?;
        let permit=Permit::new()?;
        let (finished,done)=mpsc::sync_channel(1);
        let worker = thread::Builder::new().name("sonante-pcm-guardian".into()).spawn(move || {
            let _permit=permit;
            let waits = [wait_cancel.as_raw_handle(),producer.process.as_raw_handle(),consumer.process.as_raw_handle(),ring.handles[3].as_raw_handle()];
            // A live process can still have a deadlocked consumer/control
            // loop. Check the owned controller while paused too, without UI
            // polling. The PCM producer separately bounds active backpressure.
            let result=loop {
                let result=unsafe{WaitForMultipleObjects(4,waits.as_ptr(),0,if probe.is_some(){2000}else{u32::MAX})};
                if result==windows_sys::Win32::Foundation::WAIT_TIMEOUT {
                    if probe.as_ref().is_some_and(|probe|!probe()){
                        // Explicit shutdown wins over a probe already in
                        // flight; retain ownership until the worker finishes.
                        break if unsafe{WaitForSingleObject(wait_cancel.as_raw_handle(),0)}==WAIT_OBJECT_0{WAIT_OBJECT_0}else{u32::MAX};
                    }
                }else{break result;}
            };
            // LOST/STOP retain the original continuity fault independently of
            // this cleanup acknowledgement. Returning that same fault here
            // falsely reported cleanup failure after both Jobs were dead.
            let result=super::cleanup::stop_owned(||ring.stop(result != WAIT_OBJECT_0),||producer.terminate(),||consumer.terminate());
            drop(producer); drop(consumer); drop(ring);
            let _=finished.send(result);
        }).map_err(|_| "pcm_watch_failed")?;
        Ok(Self { cancel,done,worker:Some(worker) })
    }
    pub fn shutdown(mut self) -> Result<()> {
        if unsafe { SetEvent(self.cancel.as_raw_handle()) } == 0 { return Err("pcm_signal_failed"); }
        self.join()
    }
    fn join(&mut self) -> Result<()> {
        if self.worker.is_none() { return Ok(()); }
        // Timeout never releases mapping/Job ownership or physical permit early.
        let result=self.done.recv_timeout(Duration::from_secs(5)).map_err(|_|"pcm_cleanup_pending")?;
        if let Some(worker)=self.worker.take() { if worker.is_finished() {worker.join().map_err(|_|"pcm_watch_failed")?;} }
        result
    }
}
impl Drop for Guardian { fn drop(&mut self) { if self.worker.is_none(){return;} if unsafe { SetEvent(self.cancel.as_raw_handle()) } == 0 {eprintln!("PCM: signal_failed");} if self.join().is_err() { eprintln!("PCM: cleanup_failed"); } } }
