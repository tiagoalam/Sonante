//! Fresh private staging for the original Plex A/B download budget. Call only
//! on the owned loader worker: no foreground I/O, persistent cache or user path.
use crate::windows_shared::ab_comparison as ab;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use symphonia::core::io::MediaSource;
type Result<T> = std::result::Result<T, String>;
struct Directory {
    path: PathBuf,
    anchor: Option<crate::windows_file_info::ContentDirectory>,
    _parent: crate::windows_file_info::ContentDirectory,
}
impl Drop for Directory {
    fn drop(&mut self) {
        drop(self.anchor.take());
        let _ = fs::remove_dir(&self.path); /* only this fresh EMPTY staging directory, never recursive */
    }
}
pub(crate) struct Spool {
    file: File,
    directory: Directory,
    written: u64,
    sealed: bool,
    retirement: Option<crate::windows_io::retire::Reservation<Box<dyn Send + Sync>>>,
}
fn random() -> Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|_| "AB_STORAGE")?;
    Ok(bytes.iter().map(|v| format!("{v:02x}")).collect())
}
fn create_directory(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
        let security =
            crate::windows_dsp::ring::PrivateSecurity::directory().map_err(|_| "AB_STORAGE")?;
        let sa = security.attributes();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        if unsafe { CreateDirectoryW(name.as_ptr(), &sa) } == 0 {
            return Err("AB_STORAGE".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| "AB_STORAGE".into())
    }
}
fn create_file(path: &Path) -> Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                CreateFileW, GetFileType, CREATE_NEW, DELETE, FILE_ATTRIBUTE_TEMPORARY,
                FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
                FILE_SHARE_READ, FILE_TYPE_DISK,
            },
        };
        let security =
            crate::windows_dsp::ring::PrivateSecurity::new().map_err(|_| "AB_STORAGE")?;
        let sa = security.attributes();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | DELETE,
                FILE_SHARE_READ,
                &sa,
                CREATE_NEW,
                FILE_ATTRIBUTE_TEMPORARY
                    | FILE_FLAG_DELETE_ON_CLOSE
                    | FILE_FLAG_OPEN_NO_RECALL
                    | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err("AB_STORAGE".into());
        }
        let file = unsafe { File::from_raw_handle(handle) };
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return Err("AB_STORAGE".into());
        }
        Ok(file)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| "AB_STORAGE")?;
        fs::remove_file(path).map_err(|_| "AB_STORAGE")?;
        Ok(file)
    }
}
impl Spool {
    pub(crate) fn new(parent: &Path, current: &dyn Fn() -> bool) -> Result<Self> {
        if !current() {
            return Err("AB_CHANGED".into());
        }
        let retirement =
            crate::windows_io::retire::reserve_private_preview().map_err(|_| "AB_BUSY")?;
        let parent = parent.canonicalize().map_err(|_| "AB_STORAGE")?;
        // Source NAS is supported by its own held reader; temporary writes are not
        // redirected to SMB, cloud recall, reparse paths or an arbitrary IPC path.
        if crate::windows_file_info::network_root(&parent) {
            return Err("AB_STORAGE".into());
        }
        let held =
            crate::windows_file_info::ContentDirectory::capture(&parent, &parent, || current())
                .map_err(|_| "AB_STORAGE")?;
        if !current() {
            return Err("AB_CHANGED".into());
        }
        let path = parent.join(format!("ab-preview-{}", random()?));
        create_directory(&path)?;
        let mut directory = Directory {
            path,
            anchor: None,
            _parent: held,
        };
        directory.anchor = Some(
            crate::windows_file_info::ContentDirectory::capture(&parent, &directory.path, || {
                current()
            })
            .map_err(|_| "AB_STORAGE")?,
        );
        if !current() {
            return Err("AB_CHANGED".into());
        }
        let file = create_file(&directory.path.join("sample.bin"))?;
        if !current() {
            return Err("AB_CHANGED".into());
        }
        Ok(Self {
            file,
            directory,
            written: 0,
            sealed: false,
            retirement: Some(retirement),
        })
    }
    pub(crate) fn write(&mut self, bytes: &[u8], current: &dyn Fn() -> bool) -> Result<()> {
        if self.sealed || !current() {
            return Err("AB_CHANGED".into());
        }
        let next = self
            .written
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= ab::MAX_SOURCE_BYTES as u64)
            .ok_or("AB_LIMIT")?;
        self.file.write_all(bytes).map_err(|_| "AB_STORAGE")?;
        self.written = next;
        if !current() {
            return Err("AB_CHANGED".into());
        }
        Ok(())
    }
    pub(crate) fn seal(
        mut self,
        expected: Option<u64>,
        current: &dyn Fn() -> bool,
    ) -> Result<Self> {
        if self.written == 0 || expected.is_some_and(|n| n != self.written) {
            return Err("AB_NETWORK".into());
        }
        if !current() {
            return Err("AB_CHANGED".into());
        }
        self.file.flush().map_err(|_| "AB_STORAGE")?;
        if self.file.metadata().map_err(|_| "AB_STORAGE")?.len() != self.written {
            return Err("AB_STORAGE".into());
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| "AB_STORAGE")?;
        if !current() {
            return Err("AB_CHANGED".into());
        }
        self.sealed = true;
        Ok(self)
    }
    pub(crate) fn input(mut self, alive: ab::Check, identity: String) -> ab::Input {
        let retirement = self
            .retirement
            .take()
            .expect("reserved before creating preview storage");
        let spool = std::sync::Arc::new(self);
        let guard = crate::windows_io::retire::Owned::new(
            Box::new(spool.clone()) as Box<dyn Send + Sync>,
            Some(retirement),
        );
        let media = Checked {
            spool,
            _retirement: guard,
            current: alive.clone(),
        };
        ab::Input {
            media: Box::new(media),
            alive: alive.clone(),
            validate: alive,
            identity,
        }
    }
}
struct Checked {
    // Declaration order: release direct Arc before enqueuing the last owner.
    spool: std::sync::Arc<Spool>,
    _retirement: crate::windows_io::retire::Owned<Box<dyn Send + Sync>>,
    current: ab::Check,
}
impl Checked {
    fn allowed(&self) -> std::io::Result<()> {
        if self.spool.sealed && (self.current)() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "AB_CHANGED",
            ))
        }
    }
}
impl Read for Checked {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.allowed()?;
        let n = (&self.spool.file).read(bytes)?;
        self.allowed()?;
        Ok(n)
    }
}
impl Seek for Checked {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.allowed()?;
        let at = (&self.spool.file).seek(position)?;
        self.allowed()?;
        Ok(at)
    }
}
impl MediaSource for Checked {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.spool.written)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Parent(PathBuf);
    impl Parent {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("sonante-own-ab-spool-{}", random().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Parent {
        fn drop(&mut self) {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while fs::read_dir(&self.0).unwrap().count() != 0 {
                assert!(
                    std::time::Instant::now() < end,
                    "private owner must finish physical retirement"
                );
                std::thread::yield_now();
            }

            fs::remove_dir(&self.0).unwrap();
        }
    }
    #[test]
    fn private_sealed_stream_preserves_exact_bytes_and_retires_on_drop() {
        let parent = Parent::new();
        let mut spool = Spool::new(&parent.0, &|| true).unwrap();
        spool.write(b"synthetic bytes", &|| true).unwrap();
        let mut input = spool
            .seal(Some(15), &|| true)
            .unwrap()
            .input(std::sync::Arc::new(|| true), "plex:synthetic".into());
        assert_eq!(input.media.byte_len(), Some(15));
        let mut bytes = Vec::new();
        input.media.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"synthetic bytes");
        input.media.seek(SeekFrom::Start(10)).unwrap();
        let mut tail = Vec::new();
        input.media.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, b"bytes");
        drop(input);
    }
    #[test]
    fn truncated_empty_oversized_and_cancelled_spools_are_not_inputs() {
        let parent = Parent::new();
        assert!(Spool::new(&parent.0, &|| false).is_err());
        assert!(Spool::new(&parent.0, &|| true)
            .unwrap()
            .seal(None, &|| true)
            .is_err());
        let mut spool = Spool::new(&parent.0, &|| true).unwrap();
        spool.write(b"own", &|| true).unwrap();
        assert!(spool.seal(Some(4), &|| true).is_err());
        let mut spool = Spool::new(&parent.0, &|| true).unwrap();
        spool.written = ab::MAX_SOURCE_BYTES as u64;
        assert_eq!(
            spool.write(b"x", &|| true).err().as_deref(),
            Some("AB_LIMIT")
        );
        drop(spool);
        let mut spool = Spool::new(&parent.0, &|| true).unwrap();
        assert!(spool.write(b"own", &|| false).is_err());
        assert_eq!(spool.file.metadata().unwrap().len(), 0);
    }
    #[test]
    fn cancelled_sealed_input_cannot_read_or_seek() {
        let parent = Parent::new();
        let mut spool = Spool::new(&parent.0, &|| true).unwrap();
        spool.write(b"own", &|| true).unwrap();
        let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let signal = live.clone();
        let mut input = spool.seal(None, &|| true).unwrap().input(
            std::sync::Arc::new(move || signal.load(std::sync::atomic::Ordering::Acquire)),
            "plex:own".into(),
        );
        live.store(false, std::sync::atomic::Ordering::Release);
        assert!(input.media.read(&mut [0; 1]).is_err());
        assert!(input.media.seek(SeekFrom::Start(0)).is_err());
        assert!(!(input.validate)());
    }
}
