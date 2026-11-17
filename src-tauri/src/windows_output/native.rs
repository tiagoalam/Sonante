//! Read-only WASAPI session inspection. Never activate IAudioClient, select a
//! default device, change a property/volume, register callbacks or retain COM.
use super::{acquire, owned_session, unique};
use serde::Serialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::{
    core::{Interface, PWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            Endpoints::IAudioEndpointVolume,
            eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
            IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
            StructuredStorage::{PropVariantClear, PropVariantToStringAlloc, PROPVARIANT},
            CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
        },
    },
};
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub device_name: String,
    pub observed_at: u64,
    #[serde(skip_serializing_if="Option::is_none")]
    pub endpoint_volume:Option<super::EndpointVolume>,
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
struct Allocated(PWSTR);
impl Drop for Allocated {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 .0.cast())) }
    }
}
struct Variant(PROPVARIANT);
impl Drop for Variant {
    fn drop(&mut self) {
        if unsafe { PropVariantClear(&mut self.0) }.is_err() {
            eprintln!("Windows output observation: property_cleanup_failed");
        }
    }
}
fn text(value: PWSTR, limit: usize) -> Result<String, &'static str> {
    let allocated = Allocated(value);
    if allocated.0.is_null() {
        return Err("unreadable");
    }
    // API-owned NUL-terminated allocation, bounded before copying. No raw COM
    // allocations or variants leave this apartment/thread.
    let mut len = 0;
    while len <= limit && unsafe { *allocated.0 .0.add(len) } != 0 {
        len += 1;
    }
    if len == 0 || len > limit {
        return Err("limit");
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(allocated.0 .0, len) })
        .map_err(|_| "unreadable")?;
    if value.chars().any(char::is_control) {
        return Err("unreadable");
    }
    Ok(value)
}
unsafe fn session_pid(session: &IAudioSessionControl2, pid: u32) -> Result<bool, &'static str> {
    let mut found = 0;
    // Preserve HRESULT; the generated convenience method folds every successful
    // code into Ok(pid), losing AUDCLNT_S_NO_SINGLE_PROCESS.
    let hr = unsafe {
        (Interface::vtable(session).GetProcessId)(Interface::as_raw(session), &mut found)
    };
    owned_session(hr.0, found, pid)
}
struct Candidate {
    device: IMMDevice,
    session: IAudioSessionControl2,
    id: String,
}
fn budget(deadline: Instant, current: &impl Fn() -> bool) -> Result<(), &'static str> {
    if Instant::now() >= deadline {
        return Err("timeout");
    }
    if !current() {
        return Err("changed");
    }
    Ok(())
}
unsafe fn read_endpoint_volume(device:&IMMDevice,deadline:Instant,current:&impl Fn()->bool)->Result<super::EndpointVolume,&'static str>{
    budget(deadline,current)?;
    let volume:IAudioEndpointVolume=unsafe{device.Activate(CLSCTX_ALL,None)}.map_err(|_|"unreadable")?;
    budget(deadline,current)?;
    let mut scalar=0.0;
    let gain_hr=unsafe{(Interface::vtable(&volume).GetMasterVolumeLevelScalar)(Interface::as_raw(&volume),&mut scalar)};
    if gain_hr.0!=0{return Err("unreadable");}
    budget(deadline,current)?;
    let mut muted=windows::core::BOOL::default();
    let mute_hr=unsafe{(Interface::vtable(&volume).GetMute)(Interface::as_raw(&volume),&mut muted)};
    budget(deadline,current)?;
    super::endpoint_volume(gain_hr.0,scalar,mute_hr.0,muted.as_bool())
}
pub fn observe(pid: u32, current: impl Fn() -> bool) -> Result<Observation, &'static str> {
    acquire(&current, || unsafe { inspect(pid, &current) })
}
unsafe fn inspect(pid: u32, current: &impl Fn() -> bool) -> Result<Observation, &'static str> {
    if pid == 0 {
        return Err("not_observed");
    }
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|_| "unavailable")?;
    let _apartment = Apartment;
    let deadline = Instant::now() + Duration::from_secs(2);
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|_| "unavailable")?;
    let devices = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .map_err(|_| "unreadable")?;
    let count = unsafe { devices.GetCount() }.map_err(|_| "unreadable")?;
    if count > 128 {
        return Err("limit");
    }
    let mut candidates = Vec::new();
    let mut total = 0;
    for index in 0..count {
        budget(deadline, current)?;
        let device = unsafe { devices.Item(index) }.map_err(|_| "unreadable")?;
        let manager: IAudioSessionManager2 =
            unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|_| "unreadable")?;
        let sessions = unsafe { manager.GetSessionEnumerator() }.map_err(|_| "unreadable")?;
        let sessions_count = unsafe { sessions.GetCount() }.map_err(|_| "unreadable")?;
        if sessions_count < 0 || sessions_count > 4096 {
            return Err("limit");
        }
        total += sessions_count;
        if total > 4096 {
            return Err("limit");
        }
        for session_index in 0..sessions_count {
            budget(deadline, current)?;
            let session =
                unsafe { sessions.GetSession(session_index) }.map_err(|_| "unreadable")?;
            if unsafe { session.GetState() }.map_err(|_| "unreadable")? != AudioSessionStateActive {
                continue;
            }
            let session: IAudioSessionControl2 = session.cast().map_err(|_| "unreadable")?;
            if !unsafe { session_pid(&session, pid) }? {
                continue;
            }
            let id = text(unsafe { device.GetId() }.map_err(|_| "unreadable")?, 2048)?;
            candidates.push(Candidate {
                device: device.clone(),
                session,
                id,
            });
        }
    }
    let candidate = unique(candidates)?;
    budget(deadline, current)?;
    if unsafe { candidate.device.GetState() }.map_err(|_| "unreadable")? != DEVICE_STATE_ACTIVE {
        return Err("changed");
    }
    if text(
        unsafe { candidate.device.GetId() }.map_err(|_| "unreadable")?,
        2048,
    )? != candidate.id
    {
        return Err("changed");
    }
    let store =
        unsafe { candidate.device.OpenPropertyStore(STGM_READ) }.map_err(|_| "unreadable")?;
    let property =
        Variant(unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.map_err(|_| "unreadable")?);
    let device_name = text(
        unsafe { PropVariantToStringAlloc(&property.0) }.map_err(|_| "unreadable")?,
        512,
    )?;
    // Optional point-in-time read of THIS owned session's endpoint. No default
    // endpoint lookup or volume/mute write; unsupported reads preserve the
    // previously available device-name observation without inventing a value.
    let endpoint_volume=unsafe{read_endpoint_volume(&candidate.device,deadline,current)}.ok();
    budget(deadline, current)?;
    if !unsafe { session_pid(&candidate.session, pid) }?
        || unsafe { candidate.session.GetState() }.map_err(|_| "unreadable")?
            != AudioSessionStateActive
    {
        return Err("changed");
    }
    budget(deadline, current)?;
    if unsafe { candidate.device.GetState() }.map_err(|_| "unreadable")? != DEVICE_STATE_ACTIVE {
        return Err("changed");
    }
    budget(deadline, current)?;
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "unavailable")?
        .as_millis()
        .try_into()
        .map_err(|_| "unavailable")?;
    Ok(Observation {
        device_name,
        observed_at,
        endpoint_volume,
    })
}
