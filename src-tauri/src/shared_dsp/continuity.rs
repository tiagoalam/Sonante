//! One bounded fault notification shared only with the owned EQ MPD child.
use super::Error;
use std::{fs::{self,File,OpenOptions},os::unix::fs::{FileExt,MetadataExt,OpenOptionsExt},path::{Path,PathBuf}};
pub(super) struct Marker { path:PathBuf, file:File, identity:(u64,u64), removed:bool }
impl Marker {
    pub fn create(root:&Path)->Result<Self,Error> {
        let path=root.join("continuity.state");
        let file=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&path).map_err(|_|Error::RuntimeStartFailed)?;
        let m=file.metadata().map_err(|_|Error::RuntimeStartFailed)?;
        Ok(Self{path,file,identity:(m.dev(),m.ino()),removed:false})
    }
    pub fn verify(&self)->Result<(),Error> {
        let m=fs::symlink_metadata(&self.path).map_err(|_|Error::ContinuityLost)?;
        if !m.is_file()||m.file_type().is_symlink()||(m.dev(),m.ino())!=self.identity||m.nlink()!=1 {return Err(Error::ContinuityLost)}
        let mut byte=[0];if self.file.read_at(&mut byte,0).map_err(|_|Error::ContinuityLost)?!=0{return Err(Error::ContinuityLost)}
        Ok(())
    }
    pub fn remove(&mut self)->Result<(),Error> {
        if self.removed{return Ok(())}
        match fs::symlink_metadata(&self.path) {
            Ok(m) if (m.dev(),m.ino())==self.identity&&!m.file_type().is_symlink()=>fs::remove_file(&self.path).map_err(|_|Error::CleanupFailed)?,
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            _=>return Err(Error::CleanupFailed),
        }
        self.removed=true;Ok(())
    }
}
impl Drop for Marker {fn drop(&mut self){let _=self.remove();}}
#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn fault_is_latched_and_replacement_is_not_deleted() {
  let root=std::env::temp_dir().join(format!("seq-marker-{}",std::process::id()));fs::create_dir(&root).unwrap();
  let mut m=Marker::create(&root).unwrap();m.verify().unwrap();m.file.write_at(b"fifo_stalled\n",0).unwrap();assert_eq!(m.verify(),Err(Error::ContinuityLost));
  fs::rename(&m.path,root.join("saved")).unwrap();fs::write(&m.path,b"other").unwrap();assert_eq!(m.verify(),Err(Error::ContinuityLost));assert_eq!(m.remove(),Err(Error::CleanupFailed));drop(m);
  assert_eq!(fs::read(root.join("continuity.state")).unwrap(),b"other");fs::remove_file(root.join("continuity.state")).unwrap();fs::remove_file(root.join("saved")).unwrap();fs::remove_dir(root).unwrap();
 }
}
