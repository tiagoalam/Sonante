//! Local Windows record creation with an explicit protected user/SYSTEM DACL.
//! Existing overbroad ACLs fail closed; this module never changes their ACL.
use std::{fs::File,io::Write,os::windows::{ffi::OsStrExt,io::{AsRawHandle,FromRawHandle}},path::Path,ptr::null_mut,ffi::c_void,mem::{size_of,offset_of}};
use windows_sys::Win32::{Foundation::{LocalFree,GENERIC_READ,GENERIC_WRITE,GENERIC_ALL,INVALID_HANDLE_VALUE},Security::{Authorization::{GetSecurityInfo,SE_FILE_OBJECT},GetSecurityDescriptorControl,GetSecurityDescriptorDacl,GetAce,EqualSid,ACL,ACCESS_ALLOWED_ACE,DACL_SECURITY_INFORMATION,SE_DACL_PROTECTED,IsValidAcl},Storage::FileSystem::{CreateFileW,CREATE_NEW,FILE_ATTRIBUTE_NORMAL,FILE_FLAG_OPEN_REPARSE_POINT,FILE_FLAG_OPEN_NO_RECALL,FILE_SHARE_READ,FILE_ALL_ACCESS,MoveFileExW,MOVEFILE_REPLACE_EXISTING,MOVEFILE_WRITE_THROUGH}};
const ERROR:&str="REMOTE_PREFERENCES";
struct Descriptor(*mut c_void);
impl Drop for Descriptor{fn drop(&mut self){unsafe{LocalFree(self.0);}}}
unsafe fn ace(acl:*const ACL,index:u32)->Result<(u32,*mut c_void),String>{
 if acl.is_null()||IsValidAcl(acl)==0{return Err(ERROR.into());}
 let mut raw=null_mut();if GetAce(acl,index,&mut raw)==0||raw.is_null(){return Err(ERROR.into());}
 let begin=acl as usize;let end=begin.checked_add((*acl).AclSize as usize).ok_or(ERROR)?;
 let start=raw as usize;let sid_offset=offset_of!(ACCESS_ALLOWED_ACE,SidStart);
 if start<begin+size_of::<ACL>()||start.checked_add(sid_offset+8).is_none_or(|v|v>end){return Err(ERROR.into());}
 let value=std::ptr::read_unaligned(raw.cast::<ACCESS_ALLOWED_ACE>());
 let size=value.Header.AceSize as usize;
 if value.Header.AceType!=0||value.Header.AceFlags!=0||size<sid_offset+8||start.checked_add(size).is_none_or(|v|v>end){return Err(ERROR.into());}
 let sid=raw.cast::<u8>().add(sid_offset);
 let sid_len=8+4*(*sid.add(1)as usize);
 if *sid!=1||sid_len>size-sid_offset{return Err(ERROR.into());}
 Ok((value.Mask,sid.cast()))
}
pub(super) fn local_path(path:&Path)->Result<(),String>{
 use std::path::{Component,Prefix};
 if !path.is_absolute()||!matches!(path.components().next(),Some(Component::Prefix(p))if matches!(p.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_))){return Err(ERROR.into());}Ok(())
}
pub(super) fn verify(file:&File)->Result<(),String>{
 use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle,BY_HANDLE_FILE_INFORMATION,GetFileType,FILE_TYPE_DISK};
 let mut info:BY_HANDLE_FILE_INFORMATION=unsafe{std::mem::zeroed()};
 if unsafe{GetFileType(file.as_raw_handle())}!=FILE_TYPE_DISK||unsafe{GetFileInformationByHandle(file.as_raw_handle(),&mut info)}==0||info.nNumberOfLinks!=1{return Err(ERROR.into());}
 let expected=crate::windows_dsp::ring::PrivateSecurity::new().map_err(|_|ERROR)?;
 let mut descriptor=null_mut();let mut actual=null_mut();
 if unsafe{GetSecurityInfo(file.as_raw_handle(),SE_FILE_OBJECT,DACL_SECURITY_INFORMATION,null_mut(),null_mut(),&mut actual,null_mut(),&mut descriptor)}!=0{return Err(ERROR.into());}
 let descriptor=Descriptor(descriptor);
 if descriptor.0.is_null()||actual.is_null(){return Err(ERROR.into());}
 let mut control=0;let mut revision=0;
 if unsafe{GetSecurityDescriptorControl(descriptor.0,&mut control,&mut revision)}==0||control&SE_DACL_PROTECTED==0{return Err(ERROR.into());}
 let mut present=0;let mut defaulted=0;let mut reference=null_mut();
 if unsafe{GetSecurityDescriptorDacl(expected.attributes().lpSecurityDescriptor,&mut present,&mut reference,&mut defaulted)}==0||present==0||reference.is_null(){return Err(ERROR.into());}
 unsafe{
  if IsValidAcl(actual)==0||IsValidAcl(reference)==0||(*actual).AceCount!=2||(*reference).AceCount!=2{return Err(ERROR.into());}
  let refs=[ace(reference,0)?,ace(reference,1)?];let mut used=[false;2];
  for i in 0..2{
   let(mask,sid)=ace(actual,i)?;
   if mask!=GENERIC_ALL&&mask!=FILE_ALL_ACCESS{return Err(ERROR.into());}
   let Some(n)=refs.iter().enumerate().find_map(|(n,(_,other))|(!used[n]&&EqualSid(sid,*other)!=0).then_some(n))else{return Err(ERROR.into());};used[n]=true;
  }
 }
 Ok(())
}
pub(super) fn atomic(path:&Path,bytes:&[u8])->Result<(),String>{
 local_path(path)?;
 crate::persistence::prepare_private_file_for_load(path,"remote").map_err(|_|ERROR)?;
 let mut nonce=[0;16];getrandom::fill(&mut nonce).map_err(|_|ERROR)?;
 let temp=path.parent().ok_or(ERROR)?.join(format!("remote-{nonce:x?}.tmp"));
 let from:Vec<_>=temp.as_os_str().encode_wide().chain([0]).collect();let to:Vec<_>=path.as_os_str().encode_wide().chain([0]).collect();
 let security=crate::windows_dsp::ring::PrivateSecurity::new().map_err(|_|ERROR)?;let attributes=security.attributes();
 let raw=unsafe{CreateFileW(from.as_ptr(),GENERIC_READ|GENERIC_WRITE,FILE_SHARE_READ,&attributes,CREATE_NEW,FILE_ATTRIBUTE_NORMAL|FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_OPEN_NO_RECALL,null_mut())};
 if raw==INVALID_HANDLE_VALUE{return Err(ERROR.into());}
 let result=(||{
  let mut file=unsafe{File::from_raw_handle(raw)};verify(&file)?;
  file.write_all(bytes).and_then(|_|file.sync_all()).map_err(|_|ERROR)?;drop(file);
  if unsafe{MoveFileExW(from.as_ptr(),to.as_ptr(),MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH)}==0{return Err(ERROR.into());}Ok(())
 })();
 match std::fs::remove_file(&temp){Ok(())=>{},Err(e)if e.kind()==std::io::ErrorKind::NotFound=>{},Err(_)=>return Err(ERROR.into())}result
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn native_record_is_private_and_overbroad_existing_file_is_never_repaired(){
  let mut nonce=[0;12];getrandom::fill(&mut nonce).unwrap();let root=std::env::temp_dir().join(format!("sonante-scrobble-acl-{nonce:x?}"));std::fs::create_dir(&root).unwrap();let path=root.join("record");
  atomic(&path,b"own fixture").unwrap();let file=File::open(&path).unwrap();verify(&file).unwrap();drop(file);
  // A default inherited descriptor from a temp directory is not accepted as
  // an explicit protected user/SYSTEM ACL. No mutating ACL call is made.
  let other=root.join("broad");std::fs::write(&other,b"preserve").unwrap();let file=File::open(&other).unwrap();assert!(verify(&file).is_err());drop(file);assert_eq!(std::fs::read(&other).unwrap(),b"preserve");std::fs::remove_dir_all(root).unwrap();
 }
}
