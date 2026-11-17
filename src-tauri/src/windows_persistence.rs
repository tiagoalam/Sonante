//! App-local Windows persistence adapter for stores shared with Linux.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};
const MAX_BYTES: usize = 16 * 1024 * 1024;
static ROOT: OnceLock<PathBuf> = OnceLock::new();
pub fn initialize(root: PathBuf) -> Result<(), String> {
    ROOT.set(root)
        .map_err(|_| "windows_preferences_unavailable".into())
}
pub fn sonante_config_dir() -> PathBuf {
    ROOT.get()
        .expect("Windows app data must be initialized before shared store defaults")
        .clone()
}
pub fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "windows_preferences_unavailable")?;
    let m = fs::symlink_metadata(path).map_err(|_| "windows_preferences_unavailable")?;
    if !m.is_dir() || linked(&m) {
        return Err("windows_preferences_unavailable".into());
    }
    Ok(())
}
pub fn prepare_private_file_for_load(path: &Path, label: &str) -> Result<(), String> {
    prepare_private_file_for_load_limited(path, label, MAX_BYTES as u64)
}
pub fn prepare_private_file_for_load_limited(path: &Path, _label: &str, max_bytes: u64) -> Result<(), String> {
    directory(path.parent().ok_or("windows_preferences_unavailable")?)?;
    match fs::symlink_metadata(path) {
        Ok(m) if linked(&m) || !m.is_file() || m.len() > max_bytes => {
            Err("windows_preferences_unavailable".into())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("windows_preferences_unavailable".into()),
    }
}
pub fn atomic_write_private(path: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    atomic_write_private_limited(path, bytes, label, MAX_BYTES)
}
pub fn atomic_write_private_limited(path: &Path, bytes: &[u8], label: &str, max_bytes: usize) -> Result<(), String> {
    if bytes.len() > max_bytes {
        return Err("windows_playlist_limit".into());
    }
    prepare_private_file_for_load_limited(path, label, max_bytes as u64)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "windows_preferences_unavailable")?;
    let temp = path
        .parent()
        .ok_or("windows_preferences_unavailable")?
        .join(format!("playlist-{random:x?}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| "windows_preferences_unavailable")?;
        file.write_all(bytes)
            .map_err(|_| "windows_preferences_unavailable")?;
        file.sync_all()
            .map_err(|_| "windows_preferences_unavailable")?;
        drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let from: Vec<_> = temp.as_os_str().encode_wide().chain([0]).collect();
            let to: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err("windows_preferences_unavailable".into());
            }
        }
        #[cfg(not(windows))]
        {
            fs::rename(&temp, path).map_err(|_| "windows_preferences_unavailable")?;
            fs::File::open(path.parent().ok_or("windows_preferences_unavailable")?)
                .and_then(|f| f.sync_all())
                .map_err(|_| "windows_preferences_unavailable")?;
        }
        Ok(())
    })();
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("windows_preferences_unavailable".into()),
    }
    result
}

// Each operation locks a separate file so two app instances cannot lose an
// update between the shared store's read and atomic replacement.
pub fn operation_lock(path: &Path) -> Result<fs::File, String> {
    prepare_private_file_for_load(path, "playlists.lock")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = options.open(path).map_err(|_| "windows_playlist_busy")?;
    file.try_lock().map_err(|_| "windows_playlist_busy")?;
    Ok(file)
}
