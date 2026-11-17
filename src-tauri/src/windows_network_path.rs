//! Windows network path/provider boundary. No connection, authentication prompt
//! or credential storage; resolution runs only on the bounded admission worker.
use super::{InfoError, Result};
#[cfg(windows)]use crate::diagnostics::{network_failure,NetworkStep};
use std::path::{Path, PathBuf};
#[cfg(any(windows, test))]
pub(super) fn share_allowed(server: &std::ffi::OsStr, share: &std::ffi::OsStr) -> bool {
    !server.is_empty()
        && share.to_str().is_some_and(|s| {
            !s.is_empty() && !s.eq_ignore_ascii_case("pipe") && !s.eq_ignore_ascii_case("IPC$")
        })
}
#[cfg(windows)]
pub(super) fn normalize(path: &Path) -> Result<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::ffi::{OsStrExt, OsStringExt},
        path::{Component, Prefix},
    };
    use windows_sys::Win32::NetworkManagement::WNet::{
        WNetGetUniversalNameW, UNIVERSAL_NAME_INFOW, UNIVERSAL_NAME_INFO_LEVEL,
    };
    if !super::network_root(path) {
        return Ok(path.to_path_buf());
    }
    match path.components().next() {
        Some(Component::Prefix(p))
            if matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..)) =>
        {
            return Ok(path.to_path_buf())
        }
        _ => {}
    }
    // MPR takes a drive-based path, not an extended-length device prefix.
    let text = path.to_str().ok_or(InfoError::InvalidLocalPath)?;
    let text = text.strip_prefix("\\\\?\\").unwrap_or(text);
    let wide: Vec<u16> = std::ffi::OsStr::new(text)
        .encode_wide()
        .chain([0])
        .collect();
    let mut buffer = vec![0usize; 16384]; // fixed128k / aligned for pointer header
    let mut used = (buffer.len() * std::mem::size_of::<usize>()) as u32;
    let code = unsafe {
        WNetGetUniversalNameW(
            wide.as_ptr(),
            UNIVERSAL_NAME_INFO_LEVEL,
            buffer.as_mut_ptr().cast(),
            &mut used,
        )
    };
    if code!=0{network_failure(NetworkStep::Normalize,Some(code));return Err(InfoError::Unavailable);}
    if (used as usize) > buffer.len() * std::mem::size_of::<usize>()
        || (used as usize) < std::mem::size_of::<UNIVERSAL_NAME_INFOW>()
    {
        network_failure(NetworkStep::ProviderBuffer,None);
        return Err(InfoError::Unavailable);
    }
    let info = unsafe { &*buffer.as_ptr().cast::<UNIVERSAL_NAME_INFOW>() };
    let units = bounded_name(&buffer, used as usize, info.lpUniversalName as usize)
        .ok_or_else(||{network_failure(NetworkStep::ProviderBuffer,None);InfoError::Unavailable})?;
    let path = PathBuf::from(OsString::from_wide(&units));
    if !matches!(path.components().next(),Some(Component::Prefix(p))if matches!(p.kind(),Prefix::UNC(..)|Prefix::VerbatimUNC(..)))
    {
        network_failure(NetworkStep::ProviderPath,None);
        return Err(InfoError::Unavailable);
    }
    Ok(path)
}
#[cfg(not(windows))]
pub(super) fn normalize(path: &Path) -> Result<PathBuf> {
    Ok(path.to_path_buf())
}

// The returned pointer is accepted only inside the provider's reported part of
// OUR allocation, after its pointer header, aligned and NUL-terminated. Build
// the slice from our own allocation's pointer after checking the byte offset.
#[cfg(any(windows, test))]
fn bounded_name(buffer: &[usize], used: usize, address: usize) -> Option<Vec<u16>> {
    let header = std::mem::size_of::<usize>();
    let allocated = buffer.len().checked_mul(header)?;
    if used < header || used > allocated {
        return None;
    }
    let offset = address.checked_sub(buffer.as_ptr() as usize)?;
    if offset < header || offset >= used || offset % 2 != 0 {
        return None;
    }
    let length = (used - offset) / 2;
    if length == 0 {
        return None;
    }
    let units = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
            length,
        )
    };
    let end = units.iter().position(|v| *v == 0)?;
    if end == 0 || end > 32768 {
        return None;
    }
    Some(units[..end].to_vec())
}
#[cfg(any(windows, test))]
fn protocol_allowed(version: u16, size: u16, protocol: u32, flags: u32) -> bool {
    // WNNC_NET_SMB/LANMAN; offline CSC bytes are not a live admitted share.
    // The fixed prefix is shared with FILE_REMOTE_PROTOCOL_INFORMATION; WDK
    // documents v3 (Windows8.1) and v4 (Windows10+) as well. These are header
    // versions, NOT SMB dialects. Keep the request v2 per Win32 documentation,
    // accept only known returned versions and never bypass provider/flags/ID.
    matches!(version, 1..=4) && size >= 20 && protocol == 0x00020000 && flags & 2 == 0
}
#[cfg(any(windows,test))]
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum ProtocolRefusal{Buffer,Version,Provider,Offline,Other}
/// Classify only the result of the existing guard. Do not admit any additional
/// response, retry a provider call or retain its raw fields in diagnostics.
#[cfg(any(windows,test))]
fn protocol_refusal(version:u16,size:u16,protocol:u32,flags:u32,capacity:usize)->Option<ProtocolRefusal>{
    if size as usize>capacity{return Some(ProtocolRefusal::Buffer);}
    if protocol_allowed(version,size,protocol,flags){return None;}
    Some(if size<20{ProtocolRefusal::Buffer}else if !matches!(version,1..=4){ProtocolRefusal::Version}else if protocol!=0x00020000{ProtocolRefusal::Provider}else if flags&2!=0{ProtocolRefusal::Offline}else{ProtocolRefusal::Other})
}
// The binding has only DWORD alignment. Keep the native query storage at an
// explicit 8-byte boundary; passing a field at stack+4 is possible otherwise.
// The wrapper changes neither the native structure length nor its wire fields.
#[cfg(windows)]
#[repr(C, align(8))]
struct ProtocolQueryBuffer(windows_sys::Win32::Storage::FileSystem::FILE_REMOTE_PROTOCOL_INFO);
#[cfg(windows)]
const _: () = {
    assert!(std::mem::align_of::<ProtocolQueryBuffer>() >= 8);
    assert!(std::mem::offset_of!(ProtocolQueryBuffer, 0) == 0);
    assert!(std::mem::size_of::<windows_sys::Win32::Storage::FileSystem::FILE_REMOTE_PROTOCOL_INFO>() == 116);
};
#[cfg(windows)]
pub(super) fn verify_protocol(file: &std::fs::File) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRemoteProtocolInfo, GetFileInformationByHandleEx, FILE_REMOTE_PROTOCOL_INFO,
    };
    let size = std::mem::size_of::<FILE_REMOTE_PROTOCOL_INFO>();
    let mut buffer = ProtocolQueryBuffer(FILE_REMOTE_PROTOCOL_INFO {
        StructureVersion: 2,
        StructureSize: size as u16,
        ..Default::default()
    });
    let info = &mut buffer.0;
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileRemoteProtocolInfo,
            (info as *mut FILE_REMOTE_PROTOCOL_INFO).cast(),
            size as u32,
        )
    } == 0 {network_failure(NetworkStep::ProtocolQuery,Some(unsafe{windows_sys::Win32::Foundation::GetLastError()}));return Err(InfoError::Unavailable);}
    if let Some(reason)=protocol_refusal(
            info.StructureVersion,
            info.StructureSize,
            info.Protocol,
            info.Flags,
            size,
        )
    {
        let step=match reason{ProtocolRefusal::Buffer=>NetworkStep::ProtocolBuffer,ProtocolRefusal::Version=>NetworkStep::ProtocolVersion,ProtocolRefusal::Provider=>NetworkStep::ProtocolProvider,ProtocolRefusal::Offline=>NetworkStep::ProtocolOffline,ProtocolRefusal::Other=>NetworkStep::ProtocolRejected};
        network_failure(step,None);
        return Err(InfoError::Unavailable);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn protocol_diagnostic_is_specific_without_changing_any_acceptance_result(){
        for version in [0,1,2,3,4,5,u16::MAX]{for size in [0,19,20,116,120,u16::MAX]{for protocol in [0,0x20000,0x2e0000]{for flags in [0,1,2,3,4,u32::MAX]{for capacity in [20,116,120]{assert_eq!(protocol_refusal(version,size,protocol,flags,capacity).is_none(),size as usize<=capacity&&protocol_allowed(version,size,protocol,flags));}}}}}
        assert_eq!(protocol_refusal(4,120,0x20000,0,116),Some(ProtocolRefusal::Buffer));
        assert_eq!(protocol_refusal(2,19,0x20000,0,116),Some(ProtocolRefusal::Buffer));
        assert_eq!(protocol_refusal(5,116,0x20000,0,116),Some(ProtocolRefusal::Version));
        assert_eq!(protocol_refusal(2,116,0x2e0000,0,116),Some(ProtocolRefusal::Provider));
        assert_eq!(protocol_refusal(2,116,0x20000,2,116),Some(ProtocolRefusal::Offline));
    }
    #[test]
    fn remote_pipe_namespaces_are_rejected_before_any_connection() {
        use std::ffi::OsStr;
        assert!(share_allowed(OsStr::new("server"), OsStr::new("Música")));
        for share in ["", "pipe", "PIPE", "IPC$"] {
            assert!(!share_allowed(OsStr::new("server"), OsStr::new(share)));
        }
        assert!(!share_allowed(OsStr::new(""), OsStr::new("Music")));
    }
    #[test]
    fn documented_protocol_header_versions_preserve_provider_and_offline_guards(){
        for version in 1..=4 {
            for size in [20,116,120] {assert!(protocol_allowed(version,size,0x20000,0));assert!(protocol_allowed(version,size,0x20000,1|4|8|16|32));}
            for size in [0,19]{assert!(!protocol_allowed(version,size,0x20000,0));}
            for protocol in [0,0x10000,0x2e0000,0x420000]{assert!(!protocol_allowed(version,116,protocol,0));}
            for flags in [2,3,6,u32::MAX]{assert!(!protocol_allowed(version,116,0x20000,flags));}
        }
        for version in [0,5,u16::MAX]{assert!(!protocol_allowed(version,116,0x20000,0));}
    }
    #[test]
    fn only_reported_live_smb_provider_fields_are_accepted() {
        assert!(protocol_allowed(2, 116, 0x20000, 0));
        assert!(protocol_allowed(1, 116, 0x20000, 1));
        for (version, size, protocol, flags) in [
            (0, 116, 0x20000, 0),
            (5, 116, 0x20000, 0),
            (2, 19, 0x20000, 0),
            (2, 116, 0x2e0000, 0),
            (2, 116, 0x20000, 2),
        ] {
            assert!(!protocol_allowed(version, size, protocol, flags));
        }
    }
    #[test]
    fn provider_string_must_stay_inside_the_reported_allocation() {
        let mut buffer = vec![0usize; 64];
        let header = std::mem::size_of::<usize>();
        let base = buffer.as_ptr() as usize;
        let text: Vec<u16> = "\\\\server\\share\\Música"
            .encode_utf16()
            .chain([0])
            .collect();
        unsafe {
            std::slice::from_raw_parts_mut(
                buffer.as_mut_ptr().cast::<u8>().add(header).cast::<u16>(),
                text.len(),
            )
            .copy_from_slice(&text);
        }
        let used = header + text.len() * 2;
        assert_eq!(
            bounded_name(&buffer, used, base + header),
            Some(text[..text.len() - 1].to_vec())
        );
        for address in [0, base, base + header + 1, base + used, base + used + 2] {
            assert_eq!(bounded_name(&buffer, used, address), None);
        }
        assert_eq!(bounded_name(&buffer, used - 2, base + header), None);
        assert_eq!(bounded_name(&buffer, 1024, base + header), None);
    }
}
