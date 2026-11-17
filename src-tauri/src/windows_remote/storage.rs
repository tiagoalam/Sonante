//! Private local records; never repair a corrupt or overbroad existing record.
use std::{io::Read, path::Path};
const ERROR: &str = "REMOTE_PREFERENCES";
pub(crate) fn read(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    #[cfg(windows)]
    super::private_file::local_path(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::*;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL)
            .share_mode(FILE_SHARE_READ);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return match std::fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(ERROR.into()),
            }
        }
        Err(_) => return Err(ERROR.into()),
    };
    let metadata = file.metadata().map_err(|_| ERROR)?;
    if !metadata.is_file() || crate::persistence::linked(&metadata) || metadata.len() > limit {
        return Err(ERROR.into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::*;
        if metadata.file_attributes()
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            != 0
        {
            return Err(ERROR.into());
        }
        super::private_file::verify(&file)?;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() as u64 > limit {
        return Err(ERROR.into());
    }
    Ok(Some(bytes))
}
pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 16384 {
        return Err(ERROR.into());
    }
    // Validate existing ACL and type before replacement. No normalization/reset.
    let _ = read(path, 16384)?;
    #[cfg(windows)]
    {
        super::private_file::atomic(path, bytes)
    }
    #[cfg(not(windows))]
    {
        // Portable qualifier boundary only; the Linux product never includes
        // this Windows module. Keep the original private-record 0600 contract.
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let parent = path.parent().ok_or(ERROR)?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| ERROR)?;
        let temp = parent.join(format!("remote-{nonce:x?}.tmp"));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temp)
                .map_err(|_| ERROR)?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| ERROR)?;
            drop(file);
            std::fs::rename(&temp, path).map_err(|_| ERROR)?;
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|_| ERROR)?;
            Ok(())
        })();
        match std::fs::remove_file(temp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ERROR.into()),
        }
        result
    }
}
