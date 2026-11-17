//! Windows admission/identity adapter for the unchanged bounded header readers.
//! Only opaque catalog references cross IPC. No transport, decoding or tag writes.
use crate::{
    local_file_info::{self, InfoError, LocalFileInfo},
    windows_mpd::library::{Library, Track},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
#[path="windows_file_identity.rs"]
mod identity;
pub(crate) use identity::FileIdentity;
type Result<T> = std::result::Result<T, InfoError>;
#[path="windows_path_display.rs"]
mod path_display;
pub(crate) use path_display::display as display_path;
#[path="windows_network_path.rs"]
mod network_path;
use crate::diagnostics::{network_failure,NetworkStep};
fn observed_io(step:NetworkStep,error:io::Error)->InfoError{
    network_failure(step,error.raw_os_error().and_then(|n|u32::try_from(n).ok()));io_error(error)
}
fn io_error(error: io::Error) -> InfoError {
    #[cfg(windows)]
    if error.raw_os_error() == Some(32) {
        return InfoError::Busy;
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied => InfoError::PermissionDenied,
        io::ErrorKind::NotFound => InfoError::FileNotFound,
        _ => InfoError::Unavailable,
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    identity: FileIdentity,
    len: u64,
    modified: u128,
    attributes: u32,
}
#[cfg(windows)]
fn stamp(file: &File, metadata: &Metadata) -> Result<Stamp> {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let mut info = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if !identity::legacy_query_allowed(error) {
            network_failure(NetworkStep::IdentityQuery, Some(error));
            return Err(InfoError::Unavailable);
        }
        // Only a live SMB handle can use legacy observations. Local ReFS and
        // non-SMB providers never downgrade their identity requirements.
        network_path::verify_protocol(file)?;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let mut legacy = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut legacy) } == 0 {
            network_failure(NetworkStep::IdentityQuery, Some(unsafe { windows_sys::Win32::Foundation::GetLastError() }));
            return Err(InfoError::Unavailable);
        }
        let identity = FileIdentity::smb64(legacy.dwVolumeSerialNumber, legacy.nFileIndexHigh, legacy.nFileIndexLow)
            .ok_or_else(|| { network_failure(NetworkStep::IdentityRejected,None); InfoError::Unavailable })?;
        return Ok(Stamp { identity, len: metadata.len(), modified: metadata.last_write_time().into(), attributes: metadata.file_attributes() });
    }
    if info.FileId.Identifier == [0; 16] {
        network_failure(NetworkStep::IdentityRejected,None);
        return Err(InfoError::Unavailable);
    }
    let mut bytes = [0; 24];
    bytes[..8].copy_from_slice(&info.VolumeSerialNumber.to_le_bytes());
    bytes[8..].copy_from_slice(&info.FileId.Identifier);
    Ok(Stamp {
        identity: FileIdentity::Id128(bytes),
        len: metadata.len(),
        modified: metadata.last_write_time().into(),
        attributes: metadata.file_attributes(),
    })
}
// Portable harness surrogate only. The product includes this module on Windows.
#[cfg(not(windows))]
fn stamp(_file: &File, metadata: &Metadata) -> Result<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let mut bytes = [0; 24];
    bytes[..8].copy_from_slice(&metadata.dev().to_le_bytes());
    bytes[8..16].copy_from_slice(&metadata.ino().to_le_bytes());
    Ok(Stamp {
        identity: FileIdentity::Portable(bytes),
        len: metadata.len(),
        modified: ((metadata.mtime() as u128) << 32) | (metadata.mtime_nsec() as u128),
        attributes: metadata.mode(),
    })
}
#[cfg(windows)]
fn final_path(file: &File) -> Result<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::{ffi::OsStringExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0u16; 32768];
    let count = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    } as usize;
    if count == 0 || count >= buffer.len() {
        network_failure(NetworkStep::FinalPath,if count==0{Some(unsafe{windows_sys::Win32::Foundation::GetLastError()})}else{None});
        return Err(InfoError::Unavailable);
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..count])))
}
#[cfg(not(windows))]
fn final_path(file: &File) -> Result<PathBuf> {
    use std::os::fd::AsRawFd;
    fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(io_error)
}
#[cfg(windows)]
fn local_drive(root: &Path) -> Result<()> {
    use std::path::Prefix;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    // Win32 drive type values, as declared by WindowsProgramming in the SDK.
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    let letter = match root.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(c) | Prefix::VerbatimDisk(c) => c,
            Prefix::UNC(server,share) | Prefix::VerbatimUNC(server,share) if network_path::share_allowed(server,share)=>return Ok(()),
            _ => return Err(InfoError::OutsideConfiguredRoots),
        },
        _ => return Err(InfoError::InvalidLocalPath),
    };
    let drive = [letter as u16, ':' as u16, '\\' as u16, 0];
    if !matches!(
        unsafe { GetDriveTypeW(drive.as_ptr()) },
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_REMOTE
    ) {
        return Err(InfoError::OutsideConfiguredRoots);
    }
    Ok(())
}
#[cfg(not(windows))]
fn local_drive(_root: &Path) -> Result<()> {
    Ok(())
}
pub(crate) fn content_identity(file: &File) -> Option<FileIdentity> {
    stamp(file, &file.metadata().ok()?).ok().map(|s| s.identity)
}

pub(crate) fn admit_local_root(path: &Path) -> Result<()> { local_drive(path) }
pub(crate) fn normalize_root(path:&Path)->Result<PathBuf>{
    fn check(path:&Path)->Result<()>{
        if !path.is_absolute()||path.to_str().is_none_or(|s|s.len()>32768||s.contains(['\0','\r','\n']))||path.components().any(|p|matches!(p,Component::ParentDir)){return Err(InfoError::InvalidLocalPath);}
        local_drive(path)
    }
    check(path)?;let normalized=network_path::normalize(path)?;check(&normalized)?;Ok(normalized)
}
pub(crate) fn verify_network_root(path:&Path)->Result<()>{if network_root(path){drop(ContentNode::open(path,true)?);}Ok(())}

// Detection only: this does not relax admission or promise provider support.
// Called on the bounded file-admission worker, before any media file is opened.
pub(crate) fn network_root(root:&Path)->bool{
    #[cfg(windows)]{
        use std::path::Prefix;
        match root.components().next(){
            Some(Component::Prefix(prefix))=>match prefix.kind(){
                Prefix::UNC(..)|Prefix::VerbatimUNC(..)=>true,
                Prefix::Disk(c)|Prefix::VerbatimDisk(c)=>{
                    let drive=[c as u16,':' as u16,'\\' as u16,0];
                    unsafe{windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(drive.as_ptr())==4}
                },_=>false,
            },_=>false,
        }
    }
    #[cfg(not(windows))]{let _=root;false}
}

fn allowed(metadata: &Metadata, directory: bool) -> bool {
    if if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    } {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT,
        };
        metadata.file_attributes()
            & (FILE_ATTRIBUTE_REPARSE_POINT
                | FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            == 0
    }
    #[cfg(not(windows))]
    {
        !metadata.file_type().is_symlink()
    }
}
fn open_checked(path: &Path, directory: bool) -> Result<(File, Metadata)> {
    if !allowed(&fs::symlink_metadata(path).map_err(|error|observed_io(NetworkStep::Metadata,error))?, directory) {
        network_failure(NetworkStep::Attributes,None);
        return Err(InfoError::InvalidLocalPath);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_READ,
        };
        options.share_mode(FILE_SHARE_READ).custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | FILE_FLAG_OPEN_NO_RECALL
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
    }
    let file = options.open(path).map_err(|error|observed_io(NetworkStep::Open,error))?;
    #[cfg(windows)]{
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileType,FILE_TYPE_DISK};
        if unsafe{GetFileType(file.as_raw_handle())}!=FILE_TYPE_DISK{network_failure(NetworkStep::HandleType,None);return Err(InfoError::InvalidLocalPath);}
    }
    let metadata = file.metadata().map_err(|error|observed_io(NetworkStep::Metadata,error))?;
    // The opened handle, not a prior path probe, establishes the boundary.
    if !allowed(&metadata, directory){network_failure(NetworkStep::Attributes,None);return Err(InfoError::OutsideConfiguredRoots);}
    if final_path(&file)? != path {
        network_failure(NetworkStep::PathMismatch,None);
        return Err(InfoError::OutsideConfiguredRoots);
    }
    #[cfg(windows)]
    if network_root(path){network_path::verify_protocol(&file)?;}
    Ok((file, metadata))
}
struct Held {
    path: PathBuf,
    file: File,
    stamp: Stamp,
    directory: bool,
}
impl Held {
    fn open(path: &Path, directory: bool) -> Result<Self> {
        let (file, metadata) = open_checked(path, directory)?;
        let stamp = stamp(&file, &metadata)?;
        Ok(Self {
            path: path.into(),
            file,
            stamp,
            directory,
        })
    }
    fn validate(&self) -> Result<()> {
        let current = Self::open(&self.path, self.directory).map_err(|_| InfoError::Stale)?;
        let own = stamp(&self.file, &self.file.metadata().map_err(io_error)?)?;
        if final_path(&self.file)? != self.path
            || current.stamp.identity != self.stamp.identity
            || own.identity != self.stamp.identity
            || (!self.directory && (current.stamp != self.stamp || own != self.stamp))
        {
            return Err(InfoError::Stale);
        }
        Ok(())
    }
}
struct Guard {
    nodes: Vec<Held>,
}
impl Guard {
    fn open(root: &Path, path: &Path) -> Result<Self> {
        if !root.is_absolute() || !path.is_absolute() {
            return Err(InfoError::InvalidLocalPath);
        }
        local_drive(root)?;
        let parts: Vec<_> = path
            .strip_prefix(root)
            .map_err(|_| InfoError::OutsideConfiguredRoots)?
            .components()
            .collect();
        if parts.is_empty()
            || parts.len() > 65
            || parts.iter().any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err(InfoError::InvalidLocalPath);
        }
        let mut nodes = vec![Held::open(root, true)?];
        let mut current = root.to_path_buf();
        for (i, part) in parts.iter().enumerate() {
            current.push(part.as_os_str());
            nodes.push(Held::open(&current, i + 1 < parts.len())?);
        }
        Ok(Self { nodes })
    }
    fn validate(&self) -> Result<()> {
        for node in &self.nodes {
            node.validate()?;
        }
        Ok(())
    }
    fn inspect(&mut self) -> Result<LocalFileInfo> {
        self.validate()?;
        let node = self.nodes.last_mut().ok_or(InfoError::Unavailable)?;
        let mut reader = Budget::new(&mut node.file);
        let mut info = local_file_info::read_file_info(&mut reader, node.stamp.len)?;
        self.validate()?;
        let stamp = &self.nodes.last().ok_or(InfoError::Unavailable)?.stamp;
        let mut hash = Sha256::new();
        stamp.identity.update_revision(&mut hash);
        hash.update(stamp.len.to_le_bytes());
        hash.update(stamp.modified.to_le_bytes());
        hash.update(stamp.attributes.to_le_bytes());
        info.revision = format!("{:x}", hash.finalize());
        Ok(info)
    }
}

// Local content preserves its held Win32 sharing/final-path boundary. Network
// content additionally requires a live SMB provider and scoped, typed handle IDs.
// Neither is a public/global server or physical audio-output identity claim.
struct ContentNode {
    path: PathBuf,
    file: crate::windows_io::retire::Owned<File>,
    metadata: Metadata,
    directory: bool,
    identity: Option<FileIdentity>,
}
impl ContentNode {
    fn open(path: &Path, directory: bool) -> Result<Self> {Self::open_mode(path,directory,false)}
    fn open_mode(path:&Path,directory:bool,deferred:bool)->Result<Self>{
        // Ordinary bounded content callbacks keep their exact original Drop.
        // Retained network comparison input must reserve retirement BEFORE open
        // so cancellation of an undecoded input never closes SMB on the caller.
        let reservation=if deferred&&network_root(path){Some(crate::windows_io::retire::reserve_file().map_err(|_|InfoError::Unavailable)?)}else{None};
        let (file, metadata) = open_checked(path, directory)?;
        let file=crate::windows_io::retire::Owned::new(file,reservation);
        #[cfg(windows)]
        let identity=if network_root(path){Some(stamp(&file,&metadata)?.identity)}else{None};
        #[cfg(not(windows))]
        let identity=Some(stamp(&file,&metadata)?.identity);
        Ok(Self {
            identity,
            path: path.into(),
            file,
            metadata,
            directory,
        })
    }
    fn validate(&self) -> Result<()> {
        let current = Self::open(&self.path, self.directory).map_err(|_| InfoError::Stale)?;
        let own = self.file.metadata().map_err(io_error)?;
        if !allowed(&own, self.directory) || final_path(&self.file)? != self.path {
            return Err(InfoError::Stale);
        }
        if let Some(identity)=self.identity {
            if current.identity!=Some(identity)||stamp(&self.file,&own)?.identity!=identity{return Err(InfoError::Stale);}
        }
        if !self.directory
            && (own.len() != self.metadata.len()
                || current.metadata.len() != self.metadata.len()
                || own.modified().map_err(io_error)?
                    != self.metadata.modified().map_err(io_error)?
                || current.metadata.modified().map_err(io_error)?
                    != self.metadata.modified().map_err(io_error)?)
        {
            return Err(InfoError::Stale);
        }
        Ok(())
    }
}
// Internal catalog caller supplies its current root/path and validates catalog
// epoch/membership after return. No path IPC, FileId/DAC fact or authority token.
// Win32 handles deny normal write/delete through the entire closure. Portable
// fixtures instead detect replacement by inode; that is not Windows proof.
pub(crate) fn with_bound_file<T>(
    root: &Path,
    path: &Path,
    read: impl FnOnce(File, u64) -> Result<T>,
) -> Result<T> {
    with_bound_file_while(root,path,||true,read)
}
/// The deadline/cancellation owner remains with the caller's bounded worker.
/// Check between held-node operations as well as around the content callback;
/// an old scan/admission cannot continue opening the rest of its ancestor chain.
pub(crate) fn with_bound_file_while<T>(
    root:&Path,path:&Path,current:impl Fn()->bool,
    read:impl FnOnce(File,u64)->Result<T>,
) -> Result<T> {
    let nodes=content_nodes(root,path,false,&current)?;
    let check=||if current(){Ok(())}else{Err(InfoError::Unavailable)};
    let last = nodes.last().ok_or(InfoError::Unavailable)?;
    check()?;
    let value = read(
        last.file.try_clone().map_err(io_error)?,
        last.metadata.len(),
    )?;
    check()?;
    for node in &nodes {
        check()?;
        node.validate()?;
        check()?;
    }
    Ok(value)
}

fn content_nodes(root:&Path,path:&Path,directory:bool,current:&dyn Fn()->bool)->Result<Vec<ContentNode>>{content_nodes_mode(root,path,directory,current,false)}
fn content_nodes_mode(root:&Path,path:&Path,directory:bool,current:&dyn Fn()->bool,deferred:bool)->Result<Vec<ContentNode>>{
    let check=||if current(){Ok(())}else{Err(InfoError::Unavailable)};
    check()?;
    if !root.is_absolute() || !path.is_absolute() {
        return Err(InfoError::InvalidLocalPath);
    }
    local_drive(root)?;
    check()?;
    let parts: Vec<_> = path
        .strip_prefix(root)
        .map_err(|_| InfoError::OutsideConfiguredRoots)?
        .components()
        .collect();
    if (!directory&&parts.is_empty())
        || parts.len() > 65
        || parts.iter().any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(InfoError::InvalidLocalPath);
    }
    let mut nodes = vec![ContentNode::open_mode(root, true,deferred)?];
    check()?;
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        current.push(part.as_os_str());
        check()?;
        nodes.push(ContentNode::open_mode(&current, directory || index + 1 < parts.len(),deferred)?);
        check()?;
    }
    for node in &nodes {
        check()?;
        node.validate()?;
        check()?;
    }
    Ok(nodes)
}
/// Private native source capability, never serialized or accepted from IPC.
/// Retains root, every ancestor and target with original sharing/protocol/path
/// checks. Physical validation/read/seek belong to the bounded decoder worker;
/// pure UI liveness must not call these filesystem methods.
pub(crate) struct RetainedContent {nodes:Vec<ContentNode>,len:u64}
impl RetainedContent {
    pub(crate) fn capture(root:&Path,path:&Path,current:impl Fn()->bool)->Result<std::sync::Arc<Self>>{
        let nodes=content_nodes_mode(root,path,false,&current,true)?;
        let len=nodes.last().ok_or(InfoError::Unavailable)?.metadata.len();
        Ok(std::sync::Arc::new(Self{nodes,len}))
    }
    pub(crate) fn len(&self)->u64{self.len}
    pub(crate) fn validate(&self,current:impl Fn()->bool)->Result<()>{
        for node in &self.nodes{if !current(){return Err(InfoError::Unavailable);}node.validate()?;if !current(){return Err(InfoError::Unavailable);}}Ok(())
    }
    pub(crate) fn read(&self,bytes:&mut[u8])->std::io::Result<usize>{
        use std::io::Read;let mut file=&*self.nodes.last().ok_or_else(||std::io::Error::other("AB_SOURCE"))?.file;file.read(bytes)
    }
    pub(crate) fn seek(&self,position:std::io::SeekFrom)->std::io::Result<u64>{
        use std::io::Seek;let mut file=&*self.nodes.last().ok_or_else(||std::io::Error::other("AB_SOURCE"))?.file;file.seek(position)
    }
}

// Only network scans retain this through directory enumeration. Local scans
// keep their approved behavior; this guard never escapes the physical worker.
pub(crate) struct ContentDirectory{_nodes:Vec<ContentNode>}
impl ContentDirectory{
    pub(crate) fn capture(root:&Path,path:&Path,current:impl Fn()->bool)->Result<Self>{
        Ok(Self{_nodes:content_nodes(root,path,true,&current)?})
    }
    /// Read-only observation while the complete directory ancestry is held.
    /// Used for a negative child lookup; a canonical prefix alone cannot prove
    /// absence through a reparse point or a replaced/offline network parent.
    pub(crate) fn observe<T>(root:&Path,path:&Path,current:impl Fn()->bool,read:impl FnOnce()->Result<T>)->Result<T>{
        let held=Self::capture(root,path,&current)?;
        if !current(){return Err(InfoError::Unavailable);}
        let value=read()?;
        for node in &held._nodes{
            if !current(){return Err(InfoError::Unavailable);}node.validate()?;
            if !current(){return Err(InfoError::Unavailable);}
        }Ok(value)
    }
}

#[cfg(test)]
#[path="windows_cover_read_tests.rs"]
mod cover_read_tests;

pub(crate) struct Budget<R> {
    inner: R,
    bytes: usize,
    calls: usize,
    deadline: Instant,
}
impl<R> Budget<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            bytes: 8 * 1024 * 1024,
            calls: 32768,
            deadline: Instant::now() + Duration::from_secs(2),
        }
    }
    fn step(&mut self) -> io::Result<()> {
        if self.calls == 0 || Instant::now() >= self.deadline {
            return Err(io::Error::other("header_budget"));
        }
        self.calls -= 1;
        Ok(())
    }
}
impl<R: Read> Read for Budget<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.step()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.bytes == 0 {
            return Err(io::Error::other("header_budget"));
        }
        let n = buffer.len().min(self.bytes);
        let count = self.inner.read(&mut buffer[..n])?;
        self.bytes -= count;
        Ok(count)
    }
}
impl<R: Seek> Seek for Budget<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.step()?;
        self.inner.seek(pos)
    }
}
fn valid_uri(uri: &str) -> bool {
    let p: Vec<_> = uri.split('/').collect();
    let hash = |s: &str| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit());
    p.len() == 4
        && p[0] == "windows"
        && hash(p[1])
        && hash(p[3])
        && p[2]
            .parse::<u32>()
            .is_ok_and(|d| d > 0 && d.to_string() == p[2])
}
struct Resolved {
    epoch: u64,
    root: PathBuf,
    track: Track,
}
impl Resolved {
    fn capture(library: &Library, uri: &str) -> Result<Self> {
        if !valid_uri(uri) {
            return Err(InfoError::InvalidLocalPath);
        }
        let status = library.status().map_err(|_| InfoError::Unavailable)?;
        if status.phase != "ready" {
            return Err(if status.phase == "scanning" {
                InfoError::Busy
            } else {
                InfoError::Unavailable
            });
        }
        let selected = library
            .compatibility_selection(&[uri.into()], 0)
            .map_err(|_| InfoError::Stale)?;
        Ok(Self {
            epoch: status.epoch,
            root: selected.root,
            track: selected.tracks.into_iter().next().ok_or(InfoError::Stale)?,
        })
    }
    fn validate(&self, library: &Library, uri: &str) -> Result<()> {
        let current = Self::capture(library, uri).map_err(|_| InfoError::Stale)?;
        if self.epoch != current.epoch
            || self.root != current.root
            || self.track.path != current.track.path
            || self.track.id != current.track.id
        {
            return Err(InfoError::Stale);
        }
        Ok(())
    }
}
/// Only bounded Windows metadata workers call this. The closure receives an
/// already-admitted held file and an opaque observation revision, never authority
/// for a write. Normal Win32 writes/deletes stay denied through the callback.
pub(crate) fn inspect_catalog<T>(library:&Library,uri:&str,current:impl Fn()->bool,read:impl FnOnce(&mut File,u64,String)->Result<T>)->Result<T>{
    if !current(){return Err(InfoError::Unavailable);}
    let selected=Resolved::capture(library,uri)?;
    let result=with_bound_file_while(&selected.root,&selected.track.path,&current,|mut file,len|{
        let before=stamp(&file,&file.metadata().map_err(io_error)?)?;
        let mut hash=Sha256::new();before.identity.update_revision(&mut hash);hash.update(before.len.to_le_bytes());hash.update(before.modified.to_le_bytes());hash.update(before.attributes.to_le_bytes());
        let revision=format!("windows-tags-v1.{:x}",hash.finalize());
        let value=read(&mut file,len,revision)?;
        if !current()||stamp(&file,&file.metadata().map_err(io_error)?)?!=before{return Err(InfoError::Stale);}
        Ok(value)
    })?;
    if !current(){return Err(InfoError::Unavailable);}
    selected.validate(library,uri)?;Ok(result)
}
pub fn inspect(library: &Library, uri: &str) -> Result<LocalFileInfo> {
    inspect_for(library, uri, None, None)
}
pub fn inspect_bound(library: &Library, uri: &str, expected_path: &Path, expected_identity: &FileIdentity) -> Result<LocalFileInfo> {
    inspect_for(library, uri, Some(expected_path), Some(expected_identity))
}
fn inspect_for(
    library: &Library,
    uri: &str,
    expected_path: Option<&Path>,
    expected_identity: Option<&FileIdentity>,
) -> Result<LocalFileInfo> {
    let library=library.clone();let uri=uri.to_owned();let path=expected_path.map(Path::to_path_buf);let identity=expected_identity.copied();
    crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
        if !job.current(){return Err(InfoError::Unavailable);}
        let value=inspect_read(&library,&uri,path.as_deref(),identity.as_ref());
        if !job.current(){return Err(InfoError::Unavailable);}
        value
    }).map_err(|e|if e==crate::windows_io::Error::Busy{InfoError::Busy}else{InfoError::Unavailable})?
}
fn inspect_read(library:&Library,uri:&str,expected_path:Option<&Path>,expected_identity:Option<&FileIdentity>)->Result<LocalFileInfo>{
    let selected = Resolved::capture(library, uri)?;
    if expected_path.is_some_and(|path| path != selected.track.path) {
        return Err(InfoError::Stale);
    }
    let mut guard = Guard::open(&selected.root, &selected.track.path)?;
    if expected_identity.is_some_and(|id| guard.nodes.last().is_none_or(|node| &node.stamp.identity != id)) { return Err(InfoError::Stale); }
    let info = guard.inspect()?;
    selected.validate(library, uri)?;
    guard.validate()?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_held_parent_observation_rejects_cancellation_and_never_runs_a_retired_lookup(){
        use std::cell::Cell;
        let fixture=Fixture::new();let current=Cell::new(true);
        assert!(ContentDirectory::observe(&fixture.0,&fixture.0,||false,||->Result<bool>{panic!("retired observation")}).is_err());
        assert!(ContentDirectory::observe(&fixture.0,&fixture.0,||current.get(),||{current.set(false);Ok(true)}).is_err());
        assert_eq!(ContentDirectory::observe(&fixture.0,&fixture.0,||true,||Ok(17)).unwrap(),17);
    }
    #[cfg(unix)]
    #[test]
    fn a_replaced_held_parent_cannot_publish_a_negative_lookup(){
        let fixture=Fixture::new();let sub=fixture.0.join("album");fs::create_dir(&sub).unwrap();
        assert!(ContentDirectory::observe(&fixture.0,&sub,||true,||{
            fs::rename(&sub,fixture.0.join("old-album")).unwrap();fs::create_dir(&sub).unwrap();Ok(true)
        }).is_err());
    }
    #[test]
    fn an_enumeration_guard_admits_only_its_configured_directory_chain(){
        let fixture=Fixture::new();let outside=Fixture::new();let sub=fixture.0.join("album");fs::create_dir(&sub).unwrap();
        assert!(ContentDirectory::capture(&fixture.0,&fixture.0,||true).is_ok());
        assert!(ContentDirectory::capture(&fixture.0,&sub,||true).is_ok());
        assert!(ContentDirectory::capture(&fixture.0,&outside.0,||true).is_err());
        assert!(ContentDirectory::capture(&fixture.0,&sub,||false).is_err());
        #[cfg(unix)]{let alias=fixture.0.join("alias");std::os::unix::fs::symlink(&outside.0,&alias).unwrap();assert!(ContentDirectory::capture(&fixture.0,&alias,||true).is_err());}
    }
    #[test]
    fn retired_content_admission_never_enters_its_reader_or_returns_a_late_file(){
        use std::cell::Cell;
        let fixture=Fixture::new();let path=fixture.file("retired.flac");
        assert!(with_bound_file_while(&fixture.0,&path,||false,|_,_|->Result<()>{panic!("retired reader must never run")}).is_err());
        let current=Cell::new(true);
        let result=with_bound_file_while(&fixture.0,&path,||current.get(),|file,_|{current.set(false);Ok(file)});
        assert!(matches!(result,Err(InfoError::Unavailable)));
    }
    use crate::windows_mpd::library::{track_uri, Library};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut bytes = [0; 12];
            getrandom::fill(&mut bytes).unwrap();
            let p = std::env::temp_dir().join(format!("sonante-win-info-{bytes:x?}"));
            fs::create_dir(&p).unwrap();
            Self(p.canonicalize().unwrap())
        }
        fn file(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            fs::write(&p, local_file_info::tests::fixture(96000, 24, 2, 96000)).unwrap();
            p
        }
        fn library(&self) -> Library {
            let l = Library::new(self.0.join("catalog.json"));
            l.select(self.0.clone()).unwrap();
            let end = Instant::now() + Duration::from_secs(3);
            while l.status().unwrap().phase != "ready" {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(2));
            }
            l
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn actual_original_header_reader_returns_source_facts_read_only_with_opaque_revision() {
        let fixture = Fixture::new();
        let file = fixture.file("Canção 你好.flac");
        let before = fs::read(&file).unwrap();
        let l = fixture.library();
        let albums = l.compatibility_catalog().unwrap();
        let album = albums["albums"][0]["id"].as_str().unwrap();
        let track = l.tracks(album).unwrap().remove(0);
        let uri = track_uri(&track);
        let info = inspect(&l, &uri).unwrap();
        assert_eq!(info.codec, "FLAC");
        assert_eq!(info.sample_rate, Some(96000));
        assert_eq!(info.bit_depth, Some(24));
        assert_eq!(info.duration_seconds, Some(1.0));
        assert_eq!(info.revision.len(), 64);
        assert!(info.revision.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fs::read(&file).unwrap(), before);
        l.stop();
    }
    #[test]
    fn queued_physical_path_must_match_catalog_before_any_header_read() {
        let fixture = Fixture::new();
        let path = fixture.file("a.flac");
        let library = fixture.library();
        let album = library.compatibility_catalog().unwrap()["albums"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let uri = track_uri(&library.tracks(&album).unwrap()[0]);
        let identity = content_identity(&File::open(&path).unwrap()).unwrap();
        assert!(matches!(
            inspect_bound(&library, &uri, &fixture.0.join("different.flac"), &identity),
            Err(InfoError::Stale)
        ));
        assert_eq!(inspect_bound(&library, &uri, &path, &identity).unwrap().codec, "FLAC");
        library.stop();
    }
    #[test]
    fn source_fact_read_requires_identity_of_the_actual_admitted_media_handle() {
        let fixture = Fixture::new();
        let path = fixture.file("a.flac");
        let library = fixture.library();
        let album = library.compatibility_catalog().unwrap()["albums"][0]["id"].as_str().unwrap().to_owned();
        let uri = track_uri(&library.tracks(&album).unwrap()[0]);
        let held = with_bound_file(&fixture.0, &path, |file, _| Ok(file)).unwrap();
        let identity = content_identity(&held).unwrap();
        assert_eq!(inspect_bound(&library, &uri, &path, &identity).unwrap().codec, "FLAC");
        assert!(matches!(inspect_bound(&library, &uri, &path, &FileIdentity::Id128([0; 24])), Err(InfoError::Stale)));
        #[cfg(unix)] {
            // Header/URI/path remain the same, but the opened inode is different.
            fs::rename(&path, fixture.0.join("held-old.flac")).unwrap();
            fs::write(&path, crate::local_file_info::tests::fixture(96000,24,2,96000)).unwrap();
            assert!(matches!(inspect_bound(&library, &uri, &path, &identity), Err(InfoError::Stale)));
        }
        drop(held);
        library.stop();
    }
    #[test]
    fn arbitrary_paths_traversal_ads_and_unc_never_become_authority() {
        let fixture = Fixture::new();
        fixture.file("a.flac");
        let l = fixture.library();
        for input in [
            "C:/private/a.flac",
            "../a.flac",
            "//server/music/a.flac",
            r"\\?\UNC\server\music\a.flac",
            "windows/../1/file",
            "windows/a/1/b",
            "windows/",
        ]
        .map(String::from)
        .into_iter()
        .chain([
            format!("windows/{}/01/{}", "a".repeat(64), "b".repeat(64)),
            format!("windows/{}/1/{}:stream", "a".repeat(64), "b".repeat(64)),
        ]) {
            assert!(matches!(
                inspect(&l, &input),
                Err(InfoError::InvalidLocalPath)
            ));
        }
        assert!(Guard::open(&fixture.0, &std::env::temp_dir().join("outside.flac")).is_err());
        assert!(Guard::open(&fixture.0, &fixture.0).is_err());
        l.stop();
    }
    #[test]
    fn catalog_membership_and_epoch_are_revalidated_not_only_frontend_tokens() {
        let first = Fixture::new();
        first.file("a.flac");
        let l = first.library();
        let album = l.compatibility_catalog().unwrap()["albums"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let uri = track_uri(&l.tracks(&album).unwrap()[0]);
        let resolved = Resolved::capture(&l, &uri).unwrap();
        let second = Fixture::new();
        second.file("b.flac");
        l.select(second.0.clone()).unwrap();
        assert!(matches!(resolved.validate(&l, &uri), Err(InfoError::Stale)));
        l.stop();
    }
    #[test]
    fn reader_calls_bytes_and_deadline_are_bounded_without_caching_partial_success() {
        let mut r = Budget::new(io::Cursor::new(vec![0; 8]));
        r.bytes = 2;
        let mut bytes = [0; 4];
        assert_eq!(r.read(&mut bytes).unwrap(), 2);
        assert!(r.read(&mut bytes).is_err());
        let mut r = Budget::new(io::Cursor::new(vec![0; 8]));
        r.calls = 0;
        assert!(r.seek(SeekFrom::Start(0)).is_err());
        let mut r = Budget::new(io::Cursor::new(vec![0; 8]));
        r.deadline = Instant::now();
        assert!(r.read(&mut bytes).is_err());
        let mut r = Budget::new(io::Cursor::new(local_file_info::tests::fixture(
            44100, 16, 2, 44100,
        )));
        assert_eq!(
            local_file_info::read_file_info(&mut r, 42)
                .unwrap()
                .sample_rate,
            Some(44100)
        );
        assert_eq!(r.bytes, 8 * 1024 * 1024 - 42);
    }
    #[test]
    fn missing_and_corrupt_files_are_explicit_and_never_rewritten() {
        let fixture = Fixture::new();
        let path = fixture.file("a.flac");
        assert!(matches!(
            Guard::open(&fixture.0, &fixture.0.join("missing.flac")),
            Err(InfoError::FileNotFound)
        ));
        fs::write(&path, b"fLaC\xff").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(Guard::open(&fixture.0, &path).unwrap().inspect().is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[cfg(not(windows))]
    #[test]
    fn portable_surrogate_detects_symlink_parent_replacement_and_modified_open_file() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let outside = Fixture::new();
        let file = outside.file("secret.flac");
        symlink(&outside.0, fixture.0.join("linked")).unwrap();
        assert!(Guard::open(&fixture.0, &fixture.0.join("linked/secret.flac")).is_err());
        symlink(&file, fixture.0.join("file.flac")).unwrap();
        assert!(Guard::open(&fixture.0, &fixture.0.join("file.flac")).is_err());
        let path = fixture.file("a.flac");
        let mut guard = Guard::open(&fixture.0, &path).unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(matches!(guard.inspect(), Err(InfoError::Stale)));
        drop(guard);
        fixture.file("a.flac");
        let guard = Guard::open(&fixture.0, &path).unwrap();
        fs::rename(&path, fixture.0.join("moved.flac")).unwrap();
        fixture.file("a.flac");
        assert!(matches!(guard.validate(), Err(InfoError::Stale)));
    }
    #[cfg(windows)]
    #[test]
    fn native_handles_deny_normal_write_delete_and_ancestor_rename_until_release() {
        let fixture = Fixture::new();
        let dir = fixture.0.join("album");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("a.flac");
        fs::write(&path, local_file_info::tests::fixture(44100, 16, 2, 44100)).unwrap();
        let guard = Guard::open(&fixture.0, &path).unwrap();
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(fs::remove_file(&path).is_err());
        assert!(fs::rename(&dir, fixture.0.join("renamed")).is_err());
        guard.validate().unwrap();
        drop(guard);
        assert!(OpenOptions::new().write(true).open(&path).is_ok());
    }
}

/// Finite logical sizes only, using current catalog members and held ancestors.
/// A file ID is compared only within one server/share (local IDs include volume).
pub(crate) fn catalog_sizes(library:&Library,uris:&[String],current:impl Fn()->bool)->Result<Vec<u64>>{
    use std::collections::HashSet;
    if uris.is_empty()||uris.len()>500{return Err(InfoError::Unavailable);}
    let mut held=Vec::new();let mut seen=HashSet::new();
    for uri in uris {
        if !current(){return Err(InfoError::Stale);}
        let selected=Resolved::capture(library,uri)?;
        let nodes=content_nodes(&selected.root,&selected.track.path,false,&current)?;
        let last=nodes.last().ok_or(InfoError::Unavailable)?;
        let observed=stamp(&last.file,&last.file.metadata().map_err(io_error)?)?;
        let scope=identity_scope(&selected.root)?;
        if observed.len==0||observed.len>9_007_199_254_740_991||!seen.insert((scope,observed.identity)){return Err(InfoError::Unavailable);}
        held.push((selected,nodes,observed));
    }
    let mut sizes=Vec::new();
    for ((selected,nodes,observed),uri) in held.iter().zip(uris){
        for node in nodes {if !current(){return Err(InfoError::Stale);}node.validate()?;}
        let last=nodes.last().ok_or(InfoError::Unavailable)?;
        if stamp(&last.file,&last.file.metadata().map_err(io_error)?)?!=*observed{return Err(InfoError::Stale);}
        selected.validate(library,uri)?;sizes.push(observed.len);
    }
    if !current(){return Err(InfoError::Stale);}Ok(sizes)
}
#[cfg(windows)]
fn identity_scope(root:&Path)->Result<String>{
    use std::path::Prefix;
    match root.components().next(){
        Some(Component::Prefix(p))=>match p.kind(){
            Prefix::UNC(server,share)|Prefix::VerbatimUNC(server,share)=>Ok(format!("{:?}:{:?}",server,share).to_lowercase()),
            Prefix::Disk(_)|Prefix::VerbatimDisk(_)=>if network_root(root){Err(InfoError::Unavailable)}else{Ok(String::new())},
            _=>Err(InfoError::Unavailable),
        },_=>Err(InfoError::Unavailable),
    }
}
#[cfg(not(windows))]fn identity_scope(_: &Path)->Result<String>{Ok(String::new())}
