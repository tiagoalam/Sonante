//! Filesystem scanner backends.
//!
//! Catalog logic consumes a complete `WalkResult`; platform-specific path
//! traversal stays behind this module so it cannot silently weaken catalog
//! transaction semantics.

#[cfg(target_os = "linux")]
mod platform {
    use super::super::{is_audio, is_cue, DiskFile, Result, WalkResult};
    use std::{
        ffi::{CString, OsStr, OsString},
        fs::{self, File, OpenOptions},
        io::{BufReader, Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
        path::{Component, Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::UNIX_EPOCH,
    };

    static NEXT_REPLACEMENT_ID: AtomicU64 = AtomicU64::new(0);

    const AT_FDCWD: i32 = -100;
    const O_CLOEXEC: i32 = 0x80000;
    const O_DIRECTORY: i32 = 0x10000;
    const O_NOFOLLOW: i32 = 0x20000;
    const O_PATH: i32 = 0x200000;
    const O_RDONLY: i32 = 0;
    const O_RDWR: i32 = 0x0002;
    const O_CREAT: i32 = 0x0040;
    const O_EXCL: i32 = 0x0080;
    const EEXIST: i32 = 17;

    unsafe extern "C" {
        fn openat(dirfd: i32, pathname: *const i8, flags: i32, ...) -> i32;
        fn renameat(old_dirfd: i32, old_path: *const i8, new_dirfd: i32, new_path: *const i8) -> i32;
        fn unlinkat(dirfd: i32, pathname: *const i8, flags: i32) -> i32;
        fn getrandom(buffer: *mut u8, length: usize, flags: u32) -> isize;
    }

    fn c_name(name: &OsStr) -> Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| "directory entry contains NUL byte".into())
    }

    fn descriptor_path(file: &File) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
    }

    fn open_root(root: &Path) -> Result<File> {
        let name = c_name(root.as_os_str())?;
        // O_NOFOLLOW prevents a final-component substitution. Comparing the
        // opened descriptor's procfs target with the canonical root also
        // detects a redirected ancestor between canonicalization and open.
        let fd = unsafe {
            openat(
                AT_FDCWD,
                name.as_ptr(),
                O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!(
                "cannot securely open source root: {}",
                std::io::Error::last_os_error()
            ));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let actual = fs::canonicalize(descriptor_path(&file))
            .map_err(|e| format!("cannot verify opened source root: {e}"))?;
        if actual != root {
            return Err("source root changed while opening; explicit repair required".into());
        }
        Ok(file)
    }

    fn open_child(parent: &File, name: &OsStr) -> Result<File> {
        let name = c_name(name)?;
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                O_PATH | O_NOFOLLOW | O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!(
                "directory entry changed or became inaccessible: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// O_PATH descriptors are secure anchors for traversal, but Linux rejects
    /// fsync on them with EBADF. Reopen `.` through the anchored descriptor as
    /// a syncable directory handle, without resolving the source path again.
    fn open_syncable_directory(directory: &File) -> std::io::Result<File> {
        let dot = CString::new(".").expect("static directory component has no NUL");
        let fd = unsafe {
            openat(
                directory.as_raw_fd(),
                dot.as_ptr(),
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let syncable = unsafe { File::from_raw_fd(fd) };
        let anchored = directory.metadata()?;
        let opened = syncable.metadata()?;
        if !opened.is_dir() || anchored.dev() != opened.dev() || anchored.ino() != opened.ino() {
            return Err(std::io::Error::other("anchored directory identity changed"));
        }
        Ok(syncable)
    }

    fn mtime_ns(metadata: &fs::Metadata) -> Result<i64> {
        metadata
            .modified()
            .map_err(|error| format!("cannot read modification time: {error}"))
            .map(|time| {
                time.duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
                    .unwrap_or(0)
            })
    }

    fn stamp(metadata: &fs::Metadata) -> Result<(u64, u64, i64, i64, i64)> {
        let ctime_ns = metadata
            .ctime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.ctime_nsec());
        Ok((
            metadata.dev(),
            metadata.ino(),
            i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            mtime_ns(metadata)?,
            ctime_ns,
        ))
    }

    fn open_parent(root: &Path, relative: &Path) -> Result<(File, OsString)> {
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err("unsafe relative path for secure file open".to_owned()),
            })
            .collect::<Result<Vec<_>>>()?;
        let Some((leaf, parents)) = components.split_last() else {
            return Err("empty relative path for secure file open".into());
        };

        let mut parent = open_root(root)?;
        for name in parents {
            let child = open_child(&parent, name)?;
            let metadata = child
                .metadata()
                .map_err(|error| format!("cannot inspect parent directory: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("directory path changed or contains a symlink".into());
            }
            parent = child;
        }
        Ok((parent, leaf.clone()))
    }

    fn open_child_if_present(parent: &File, name: &OsStr) -> Result<Option<File>> {
        let name = c_name(name)?;
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                O_PATH | O_NOFOLLOW | O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(format!("cannot securely open cover entry: {error}"));
        }
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }

    fn open_parent_if_present(root: &Path, relative: &Path) -> Result<Option<(File, OsString)>> {
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err("unsafe relative path for secure cover open".to_owned()),
            })
            .collect::<Result<Vec<_>>>()?;
        let Some((leaf, parents)) = components.split_last() else {
            return Err("empty relative path for secure cover open".into());
        };

        let mut parent = open_root(root)?;
        for name in parents {
            let Some(child) = open_child_if_present(&parent, name)? else {
                return Ok(None);
            };
            let metadata = child
                .metadata()
                .map_err(|error| format!("cannot inspect cover directory: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("cover path changed or contains a symlink".into());
            }
            parent = child;
        }
        Ok(Some((parent, leaf.clone())))
    }

    /// Read a bounded regular file through descriptor-anchored directories.
    /// Missing candidates are ordinary; symlinks, special files, oversized
    /// files, and files changed while being read are rejected.
    pub(crate) fn read_relative_file(
        root: &Path,
        relative: &Path,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>> {
        let Some((parent, leaf)) = open_parent_if_present(root, relative)? else {
            return Ok(None);
        };
        let Some(path_handle) = open_child_if_present(&parent, &leaf)? else {
            return Ok(None);
        };
        let before = path_handle
            .metadata()
            .map_err(|error| format!("cannot inspect cover file: {error}"))?;
        if before.file_type().is_symlink() {
            return Err("cover entry is a symlink".into());
        }
        if !before.is_file() {
            return Err("cover entry is not a regular file".into());
        }
        if before.len() > max_bytes as u64 {
            return Err("cover file exceeds size limit".into());
        }
        let expected = stamp(&before)?;
        let mut readable = OpenOptions::new()
            .read(true)
            .custom_flags(O_CLOEXEC | 0x800)
            .open(descriptor_path(&path_handle))
            .map_err(|error| format!("cannot read validated cover file: {error}"))?;
        if stamp(
            &readable
                .metadata()
                .map_err(|error| format!("cannot inspect opened cover file: {error}"))?,
        )? != expected
        {
            return Err("cover file identity changed while opening".into());
        }

        let mut bytes = Vec::with_capacity(before.len().min(max_bytes as u64) as usize);
        Read::by_ref(&mut readable)
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read cover file: {error}"))?;
        if bytes.len() > max_bytes {
            return Err("cover file grew beyond size limit".into());
        }
        let opened_after = path_handle
            .metadata()
            .map_err(|error| format!("cannot recheck opened cover file: {error}"))?;
        let entry_after = open_child_if_present(&parent, &leaf)?
            .ok_or_else(|| "cover entry disappeared while being read".to_owned())?
            .metadata()
            .map_err(|error| format!("cannot recheck cover directory entry: {error}"))?;
        if stamp(&opened_after)? != expected
            || stamp(&entry_after)? != expected
            || entry_after.file_type().is_symlink()
            || !entry_after.is_file()
        {
            return Err("cover file changed during read".into());
        }
        Ok(Some(bytes))
    }

    /// Open a previously enumerated regular file relative to descriptor-anchored
    /// directories, parse it while the handle is alive, and reject the result if
    /// the opened object or its directory entry changed during the read.
    pub(crate) fn with_open_file<T, F>(root: &Path, expected: &DiskFile, parse: F) -> Result<T>
    where
        F: FnOnce(BufReader<File>) -> T,
    {
        let (parent, leaf) = open_parent(root, &expected.relative)?;
        let path_handle = open_child(&parent, &leaf)?;
        let before = path_handle
            .metadata()
            .map_err(|error| format!("cannot inspect audio file: {error}"))?;
        if before.file_type().is_symlink() || !before.is_file() {
            return Err("audio entry is no longer a regular file".into());
        }
        let expected_stamp = (
            expected.device,
            expected.inode,
            expected.size,
            expected.mtime_ns,
            expected.ctime_ns,
        );
        if stamp(&before)? != expected_stamp {
            return Err("audio file changed after filesystem enumeration".into());
        }

        // Reopen the already-validated O_PATH descriptor, not the user path.
        // O_NONBLOCK prevents a replaced FIFO from hanging even if the procfs
        // descriptor semantics are unexpectedly unavailable.
        let readable = OpenOptions::new()
            .read(true)
            .custom_flags(O_CLOEXEC | 0x800) // O_NONBLOCK
            .open(descriptor_path(&path_handle))
            .map_err(|error| format!("cannot open validated audio file: {error}"))?;
        let readable_before = readable
            .metadata()
            .map_err(|error| format!("cannot inspect opened audio file: {error}"))?;
        if stamp(&readable_before)? != expected_stamp {
            return Err("audio file identity changed while opening for parsing".into());
        }

        let result = parse(BufReader::new(readable));

        let opened_after = path_handle
            .metadata()
            .map_err(|error| format!("cannot recheck opened audio file: {error}"))?;
        let entry_after = open_child(&parent, &leaf).and_then(|entry| {
            entry
                .metadata()
                .map_err(|error| format!("cannot recheck audio directory entry: {error}"))
        })?;
        if stamp(&opened_after)? != expected_stamp
            || stamp(&entry_after)? != expected_stamp
            || entry_after.file_type().is_symlink()
            || !entry_after.is_file()
        {
            return Err("audio file changed during metadata extraction".into());
        }
        Ok(result)
    }

    /// Open a cataloged file through descriptor-anchored directories for a
    /// bounded on-demand read. The catalog persists only size/mtime, so bind
    /// this read to those values and then retain the complete observed stamp
    /// for the lifetime of the descriptor. The path is never reopened by name.
    pub(crate) fn with_verified_file<T, F>(
        root: &Path,
        relative: &Path,
        expected_size: i64,
        expected_mtime_ns: i64,
        read: F,
    ) -> Result<T>
    where
        F: FnOnce(BufReader<File>, u64) -> T,
    {
        let (parent, leaf) = open_parent(root, relative)?;
        let path_handle = open_child(&parent, &leaf)?;
        let before = path_handle
            .metadata()
            .map_err(|error| format!("cannot inspect cataloged file: {error}"))?;
        if before.file_type().is_symlink() || !before.is_file() {
            return Err("cataloged entry is no longer a regular file".into());
        }
        let expected = stamp(&before)?;
        if expected.2 != expected_size || expected.3 != expected_mtime_ns {
            return Err("cataloged file changed before on-demand read".into());
        }
        let readable = OpenOptions::new()
            .read(true)
            .custom_flags(O_CLOEXEC | 0x800)
            .open(descriptor_path(&path_handle))
            .map_err(|error| format!("cannot open validated cataloged file: {error}"))?;
        if stamp(
            &readable
                .metadata()
                .map_err(|error| format!("cannot inspect opened file: {error}"))?,
        )? != expected
        {
            return Err("cataloged file identity changed while opening".into());
        }

        let length = expected.2.max(0) as u64;
        let result = read(BufReader::new(readable), length);
        let opened_after = path_handle
            .metadata()
            .map_err(|error| format!("cannot recheck opened cataloged file: {error}"))?;
        let entry_after = open_child(&parent, &leaf).and_then(|entry| {
            entry
                .metadata()
                .map_err(|error| format!("cannot recheck cataloged directory entry: {error}"))
        })?;
        if stamp(&opened_after)? != expected
            || stamp(&entry_after)? != expected
            || entry_after.file_type().is_symlink()
            || !entry_after.is_file()
        {
            return Err("cataloged file changed during on-demand read".into());
        }
        Ok(result)
    }

    /// Return a retained descriptor for a cataloged regular file. The
    /// descriptor is opened beneath anchored directory handles and its full
    /// observed identity is checked before returning. Consumers keep this
    /// handle instead of reopening the path; `identity_matches` can then
    /// detect in-place mutation while the handle remains alive.
    pub(crate) fn open_verified_file(
        root: &Path,
        relative: &Path,
        expected_size: i64,
        expected_mtime_ns: i64,
    ) -> Result<(File, (u64, u64, i64, i64, i64))> {
        with_verified_file(root, relative, expected_size, expected_mtime_ns, |reader, _| {
            let metadata = reader.get_ref().metadata()
                .map_err(|error| format!("cannot inspect opened cataloged file: {error}"))?;
            let identity = stamp(&metadata)?;
            let file = reader.get_ref().try_clone()
                .map_err(|error| format!("cannot retain opened cataloged file: {error}"))?;
            Ok((file, identity))
        })?
    }

    pub(crate) fn identity_matches(
        file: &File,
        expected: (u64, u64, i64, i64, i64),
    ) -> bool {
        file.metadata().ok().and_then(|metadata| stamp(&metadata).ok()) == Some(expected)
    }

    struct ReplacementTemp {
        directory: File,
        name: CString,
        file: Option<File>,
        committed: bool,
    }

    impl Drop for ReplacementTemp {
        fn drop(&mut self) {
            self.file.take();
            if !self.committed {
                unsafe {
                    unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0);
                }
            }
        }
    }

    /// Replace one cataloged file through its anchored parent descriptor.
    /// The transformation callback must validate its caller's revision and
    /// preserve required file metadata. This function owns the final identity
    /// checks, sibling staging file, atomic rename, and directory sync.
    pub(crate) fn replace_verified_file<T, F>(
        root: &Path,
        relative: &Path,
        expected_size: i64,
        expected_mtime_ns: i64,
        transform: F,
    ) -> std::result::Result<super::super::TrackFileReplaceSuccess<T>, super::super::TrackFileReplaceFailure>
    where
        F: FnOnce(&mut File, &mut File, &fs::Metadata) -> std::result::Result<T, String>,
    {
        let fail = |message: String, committed| super::super::TrackFileReplaceFailure {
            message,
            committed,
        };
        let (directory, leaf) = open_parent(root, relative).map_err(|error| fail(error, false))?;
        let original_name = CString::new(leaf.as_bytes())
            .map_err(|_| fail("unsafe file name for replacement".into(), false))?;
        let path_handle = open_child(&directory, &leaf).map_err(|error| fail(error, false))?;
        let before = path_handle
            .metadata()
            .map_err(|error| fail(format!("cannot inspect file before replacement: {error}"), false))?;
        if before.file_type().is_symlink() || !before.is_file() {
            return Err(fail("cataloged entry is no longer a regular file".into(), false));
        }
        let expected = stamp(&before).map_err(|error| fail(error, false))?;
        if expected.2 != expected_size || expected.3 != expected_mtime_ns {
            return Err(fail("cataloged file changed before tag edit".into(), false));
        }
        let mut original = OpenOptions::new()
            .read(true)
            .custom_flags(O_CLOEXEC | 0x800)
            .open(descriptor_path(&path_handle))
            .map_err(|error| fail(format!("cannot open validated file: {error}"), false))?;
        if stamp(&original.metadata().map_err(|error| fail(format!("cannot inspect opened file: {error}"), false))?)
            .map_err(|error| fail(error, false))? != expected
        {
            return Err(fail("file identity changed while opening for tag edit".into(), false));
        }

        let (name, file) = create_replacement_temp(&directory, &original_name)
            .map_err(|error| fail(error, false))?;
        let mut temp = ReplacementTemp {
            directory,
            name,
            file: Some(file),
            committed: false,
        };
        let temp_file = temp.file.as_mut()
            .ok_or_else(|| fail("staged tag file is unavailable".into(), false))?;
        let transformed = transform(&mut original, temp_file, &before)
        .map_err(|error| fail(error, false))?;
        let temp_file = temp.file.as_mut()
            .ok_or_else(|| fail("staged tag file is unavailable".into(), false))?;
        temp_file
            .flush()
            .map_err(|error| fail(format!("cannot flush staged file: {error}"), false))?;
        temp_file
            .sync_all()
            .map_err(|error| fail(format!("cannot sync staged file: {error}"), false))?;
        let staged_file = temp.file.as_ref()
            .ok_or_else(|| fail("staged tag file is unavailable".into(), false))?;
        let staged = staged_file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect staged file: {error}"), false))?;
        if !staged.is_file() {
            return Err(fail("staged replacement is not a regular file".into(), false));
        }

        let current = path_handle
            .metadata()
            .map_err(|error| fail(format!("cannot recheck original file: {error}"), false))?;
        let entry = open_child(&temp.directory, &leaf)
            .and_then(|entry| entry.metadata().map_err(|error| format!("cannot recheck original directory entry: {error}")))
            .map_err(|error| fail(error, false))?;
        let (current_parent, current_leaf) = open_parent(root, relative).map_err(|error| fail(error, false))?;
        let old_parent_meta = temp.directory.metadata().map_err(|error| fail(format!("cannot inspect source directory: {error}"), false))?;
        let new_parent_meta = current_parent.metadata().map_err(|error| fail(format!("cannot recheck source directory: {error}"), false))?;
        let current_leaf_c = CString::new(current_leaf.as_bytes()).map_err(|_| fail("unsafe file name for replacement".into(), false))?;
        let current_named = open_child(&current_parent, &current_leaf)
            .and_then(|entry| entry.metadata().map_err(|error| format!("cannot recheck current file entry: {error}")))
            .map_err(|error| fail(error, false))?;
        if old_parent_meta.dev() != new_parent_meta.dev()
            || old_parent_meta.ino() != new_parent_meta.ino()
            || stamp(&current).map_err(|error| fail(error, false))? != expected
            || stamp(&entry).map_err(|error| fail(error, false))? != expected
            || stamp(&current_named).map_err(|error| fail(error, false))? != expected
            || entry.file_type().is_symlink()
            || !entry.is_file()
        {
            return Err(fail("file or source directory changed during tag edit".into(), false));
        }

        // Prepare a syncable directory descriptor before rename. The existing
        // traversal descriptor is O_PATH and cannot be fsynced on Linux.
        let sync_directory = open_syncable_directory(&temp.directory)
            .map_err(|error| fail(format!("cannot prepare source directory sync: {error}"), false))?;

        let rename_result = unsafe {
            renameat(
                temp.directory.as_raw_fd(),
                temp.name.as_ptr(),
                current_parent.as_raw_fd(),
                current_leaf_c.as_ptr(),
            )
        };
        if rename_result != 0 {
            return Err(fail(format!("cannot atomically replace tag file: {}", std::io::Error::last_os_error()), false));
        }
        temp.committed = true;
        if let Err(error) = sync_directory.sync_all() {
            return Err(fail(format!("tag file replaced but directory durability is unconfirmed: {error}"), true));
        }
        let metadata = temp.file.as_ref()
            .ok_or_else(|| fail("tag file replaced but staged handle is unavailable".into(), true))?
            .metadata()
            .map_err(|error| fail(format!("tag file replaced but final metadata is unavailable: {error}"), true))?;
        Ok(super::super::TrackFileReplaceSuccess { value: transformed, metadata })
    }

    /// Conservatively detect a CUE sheet beside an indexed audio member. The
    /// directory is enumerated through its already-open no-follow descriptor,
    /// never by reconstructing a path from the source root.
    pub(crate) fn directory_has_cue(root: &Path, relative: &Path) -> Result<bool> {
        let (directory, _) = open_parent(root, relative)?;
        for entry in fs::read_dir(descriptor_path(&directory))
            .map_err(|error| format!("cannot inspect track directory for CUE sheets: {error}"))?
        {
            let entry = entry.map_err(|error| format!("cannot inspect track directory entry: {error}"))?;
            if !entry.file_type().map_err(|error| format!("cannot inspect track entry type: {error}"))?.is_file() {
                continue;
            }
            if entry.path().extension().and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("cue")) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn create_replacement_temp(directory: &File, _original_name: &CString) -> Result<(CString, File)> {
        for _ in 0..16 {
            let id = NEXT_REPLACEMENT_ID.fetch_add(1, Ordering::Relaxed);
            let mut random = [0_u8; 16];
            let read = unsafe { getrandom(random.as_mut_ptr(), random.len(), 0) };
            if read != random.len() as isize {
                return Err("cannot obtain randomness for staged tag file".into());
            }
            let suffix = random.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            let name = CString::new(format!(".sonante-sqlite-tags-{}-{id}-{suffix}.tmp", std::process::id()))
                .map_err(|_| "invalid generated temporary file name".to_owned())?;
            let fd = unsafe {
                openat(directory.as_raw_fd(), name.as_ptr(),
                    O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0o600)
            };
            if fd >= 0 {
                return Ok((name, unsafe { File::from_raw_fd(fd) }));
            }
            if std::io::Error::last_os_error().raw_os_error() != Some(EEXIST) {
                return Err("cannot create staged tag file in source directory".into());
            }
        }
        Err("cannot allocate a unique staged tag file".into())
    }

    /// Verify that a cataloged file is still a regular file reachable below
    /// the anchored source root. The returned path is for consumers such as
    /// MPD that must open the file themselves; callers should keep the check
    /// immediately adjacent to handing the path to that consumer.
    pub(crate) fn verify_relative_file(
        root: &Path,
        relative: &Path,
        expected_size: i64,
        expected_mtime_ns: i64,
    ) -> Result<PathBuf> {
        let (parent, leaf) = open_parent(root, relative)?;
        let path_handle = open_child(&parent, &leaf)?;
        let before = path_handle
            .metadata()
            .map_err(|error| format!("cannot inspect audio file: {error}"))?;
        if before.file_type().is_symlink() || !before.is_file() {
            return Err("audio entry is no longer a regular file".into());
        }
        let expected = stamp(&before)?;
        if expected.2 != expected_size || expected.3 != expected_mtime_ns {
            return Err("audio file changed since it was indexed".into());
        }

        // Opening the descriptor target confirms the current object is
        // readable without resolving the user supplied path a second time.
        let readable = OpenOptions::new()
            .read(true)
            .custom_flags(O_CLOEXEC | 0x800)
            .open(descriptor_path(&path_handle))
            .map_err(|error| format!("cannot open validated audio file: {error}"))?;
        if stamp(
            &readable
                .metadata()
                .map_err(|error| format!("cannot inspect opened audio file: {error}"))?,
        )? != expected
        {
            return Err("audio file identity changed while opening".into());
        }
        let entry_after = open_child(&parent, &leaf)?
            .metadata()
            .map_err(|error| format!("cannot recheck audio directory entry: {error}"))?;
        if stamp(&entry_after)? != expected
            || entry_after.file_type().is_symlink()
            || !entry_after.is_file()
        {
            return Err("audio file changed while validating playback".into());
        }
        Ok(root.join(relative))
    }

    fn walk_dir<
        F: FnMut(&Path) -> Result<()>,
        G: FnMut(&Path) -> Result<()>,
        H: FnMut(usize, usize, usize),
    >(
        dir: &File,
        relative_dir: &Path,
        out: &mut WalkResult,
        before_open: &mut F,
        before_metadata: &mut G,
        progress: &mut H,
    ) -> Result<()> {
        // Enumerate through the already-open directory descriptor. Every name
        // is then resolved relative to that descriptor with O_NOFOLLOW.
        let entries = fs::read_dir(descriptor_path(dir))
            .map_err(|e| format!("cannot enumerate opened directory: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("directory entry failed: {e}"))?;
            let name = entry.file_name();
            let relative = relative_dir.join(&name);
            if !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            {
                return Err("unsafe relative path from directory entry".into());
            }
            before_open(&relative)?;
            let opened = open_child(dir, &name)?;
            before_metadata(&relative)?;
            let metadata = opened
                .metadata()
                .map_err(|e| format!("cannot inspect opened entry: {e}"))?;
            out.entries += 1;
            if out.entries % 64 == 0 {
                progress(out.entries, out.files_examined, out.files.len());
            }

            // O_PATH|O_NOFOLLOW opens a symlink itself on Linux. It is
            // intentionally ignored and never resolved to its target.
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                walk_dir(
                    &opened,
                    &relative,
                    out,
                    before_open,
                    before_metadata,
                    progress,
                )?;
                continue;
            }
            if !metadata.is_file() {
                continue;
            }

            out.files_examined += 1;
            let size = i64::try_from(metadata.len())
                .map_err(|_| "file too large for SQLite integer".to_string())?;
            let (device, inode, _, mtime_ns, ctime_ns) = stamp(&metadata)?;
            let file = DiskFile {
                relative: relative.clone(),
                size,
                mtime_ns,
                device,
                inode,
                ctime_ns,
            };
            if is_audio(&relative) {
                out.files.push(file);
            } else if is_cue(&relative) {
                out.cue_files.push(file);
            }
        }
        Ok(())
    }

    fn walk_impl<F, G, H>(
        root: &Path,
        mut before_open: F,
        mut before_metadata: G,
        mut progress: H,
    ) -> Result<WalkResult>
    where
        F: FnMut(&Path) -> Result<()>,
        G: FnMut(&Path) -> Result<()>,
        H: FnMut(usize, usize, usize),
    {
        let root_fd = open_root(root)?;
        let mut out = WalkResult::default();
        walk_dir(
            &root_fd,
            Path::new(""),
            &mut out,
            &mut before_open,
            &mut before_metadata,
            &mut progress,
        )?;
        progress(out.entries, out.files_examined, out.files.len());
        out.files.sort_by(|a, b| a.relative.cmp(&b.relative));
        out.cue_files.sort_by(|a, b| a.relative.cmp(&b.relative));
        Ok(out)
    }

    pub(crate) fn walk(root: &Path) -> Result<WalkResult> {
        walk_impl(root, |_| Ok(()), |_| Ok(()), |_, _, _| {})
    }

    pub(crate) fn walk_with_progress<F>(root: &Path, progress: F) -> Result<WalkResult>
    where
        F: FnMut(usize, usize, usize),
    {
        walk_impl(root, |_| Ok(()), |_| Ok(()), progress)
    }

    #[cfg(test)]
    pub(crate) fn walk_with_hook<F: FnMut(&Path)>(root: &Path, mut hook: F) -> Result<WalkResult> {
        walk_impl(
            root,
            |path| {
                hook(path);
                Ok(())
            },
            |_| Ok(()),
            |_, _, _| {},
        )
    }

    #[cfg(test)]
    pub(crate) fn walk_with_failure_after(root: &Path, entry_count: usize) -> Result<WalkResult> {
        let mut visited = 0;
        walk_impl(
            root,
            |_| {
                visited += 1;
                if visited == entry_count {
                    Err(format!(
                        "injected directory enumeration error after {} entries",
                        visited - 1
                    ))
                } else {
                    Ok(())
                }
            },
            |_| Ok(()),
            |_, _, _| {},
        )
    }

    #[cfg(test)]
    pub(crate) fn walk_with_metadata_failure(
        root: &Path,
        failed_relative: &Path,
    ) -> Result<WalkResult> {
        walk_impl(
            root,
            |_| Ok(()),
            |relative| {
                if relative == failed_relative {
                    Err("injected file metadata error".into())
                } else {
                    Ok(())
                }
            },
            |_, _, _| {},
        )
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::super::DiskFile;
    use super::super::{Result, WalkResult};
    use std::path::{Path, PathBuf};
    use std::{fs::File, io::BufReader};

    pub(crate) fn walk(_root: &Path) -> Result<WalkResult> {
        Err("secure descriptor-anchored scanning is currently supported only on Linux".into())
    }

    pub(crate) fn walk_with_progress<F>(_root: &Path, _progress: F) -> Result<WalkResult>
    where
        F: FnMut(usize, usize, usize),
    {
        Err("secure descriptor-anchored scanning is currently supported only on Linux".into())
    }

    pub(crate) fn with_open_file<T, F>(_root: &Path, _expected: &DiskFile, _parse: F) -> Result<T>
    where
        F: FnOnce(BufReader<File>) -> T,
    {
        Err("secure descriptor-anchored file opening is currently supported only on Linux".into())
    }

    pub(crate) fn with_verified_file<T, F>(
        _root: &Path,
        _relative: &Path,
        _expected_size: i64,
        _expected_mtime_ns: i64,
        _read: F,
    ) -> Result<T>
    where
        F: FnOnce(BufReader<File>, u64) -> T,
    {
        Err("secure descriptor-anchored file opening is currently supported only on Linux".into())
    }

    pub(crate) fn open_verified_file(
        _root: &Path,
        _relative: &Path,
        _expected_size: i64,
        _expected_mtime_ns: i64,
    ) -> Result<(File, (u64, u64, i64, i64, i64))> {
        Err("secure descriptor-anchored file opening is currently supported only on Linux".into())
    }

    pub(crate) fn identity_matches(
        _file: &File,
        _expected: (u64, u64, i64, i64, i64),
    ) -> bool { false }

    pub(crate) fn verify_relative_file(
        _root: &Path,
        _relative: &Path,
        _expected_size: i64,
        _expected_mtime_ns: i64,
    ) -> Result<PathBuf> {
        Err("secure descriptor-anchored file checks are currently supported only on Linux".into())
    }

    pub(crate) fn directory_has_cue(_root: &Path, _relative: &Path) -> Result<bool> {
        Err("secure CUE checks are currently supported only on Linux".into())
    }

    pub(crate) fn replace_verified_file<T, F>(
        _root: &Path,
        _relative: &Path,
        _expected_size: i64,
        _expected_mtime_ns: i64,
        _transform: F,
    ) -> std::result::Result<super::super::TrackFileReplaceSuccess<T>, super::super::TrackFileReplaceFailure>
    where
        F: FnOnce(&mut File, &mut File, &std::fs::Metadata) -> std::result::Result<T, String>,
    {
        Err(super::super::TrackFileReplaceFailure {
            message: "secure descriptor-anchored file replacement is currently supported only on Linux".into(),
            committed: false,
        })
    }

    pub(crate) fn read_relative_file(
        _root: &Path,
        _relative: &Path,
        _max_bytes: usize,
    ) -> Result<Option<Vec<u8>>> {
        Err("secure descriptor-anchored file reads are currently supported only on Linux".into())
    }
}

pub(crate) use platform::walk_with_progress;
pub(crate) use platform::{
    directory_has_cue, read_relative_file, replace_verified_file, verify_relative_file, walk, with_open_file,
    with_verified_file, open_verified_file, identity_matches,
};

#[cfg(all(test, target_os = "linux"))]
pub(crate) use platform::walk_with_hook;

#[cfg(all(test, target_os = "linux"))]
pub(crate) use platform::{walk_with_failure_after, walk_with_metadata_failure};

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Read,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sonante-sqlite-scanner-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn descriptor_scan_enumerates_normal_nested_directories() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root/nested")).unwrap();
        fs::write(temp.path("root/one.flac"), b"one").unwrap();
        fs::write(temp.path("root/nested/two.wav"), b"two").unwrap();
        fs::write(temp.path("root/ignore.txt"), b"other").unwrap();

        let result = walk(&fs::canonicalize(temp.path("root")).unwrap()).unwrap();
        assert_eq!(
            result
                .files
                .iter()
                .map(|f| f.relative.as_path())
                .collect::<Vec<_>>(),
            [Path::new("nested/two.wav"), Path::new("one.flac")]
        );
        assert_eq!(result.files_examined, 3);
    }

    #[test]
    fn static_internal_and_external_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root/inside")).unwrap();
        fs::create_dir_all(temp.path("outside")).unwrap();
        fs::write(temp.path("root/inside/internal.flac"), b"inside").unwrap();
        fs::write(temp.path("outside/external.flac"), b"outside").unwrap();
        symlink(temp.path("root/inside"), temp.path("root/internal-link")).unwrap();
        symlink(temp.path("outside"), temp.path("root/external-link")).unwrap();
        symlink(
            temp.path("outside/external.flac"),
            temp.path("root/external-file.flac"),
        )
        .unwrap();

        let result = walk(&fs::canonicalize(temp.path("root")).unwrap()).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].relative, Path::new("inside/internal.flac"));
    }

    #[test]
    fn replacing_a_listed_directory_with_external_symlink_is_not_followed() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root/swap")).unwrap();
        fs::create_dir_all(temp.path("outside")).unwrap();
        fs::write(temp.path("outside/escape.flac"), b"outside").unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();
        let moved = temp.path("outside/moved-original");
        let link_target = temp.path("outside");

        let result = walk_with_hook(&root, |relative| {
            if relative == Path::new("swap") && temp.path("root/swap").is_dir() {
                fs::rename(temp.path("root/swap"), &moved).unwrap();
                symlink(&link_target, temp.path("root/swap")).unwrap();
            }
        })
        .unwrap();
        assert!(!result
            .files
            .iter()
            .any(|file| file.relative == Path::new("swap/escape.flac")));
    }

    #[test]
    fn directory_removed_after_enumeration_fails_the_whole_scan() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root/gone")).unwrap();
        fs::write(temp.path("root/gone/track.flac"), b"track").unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();

        let result = walk_with_hook(&root, |relative| {
            if relative == Path::new("gone") && temp.path("root/gone").exists() {
                fs::remove_dir_all(temp.path("root/gone")).unwrap();
            }
        });
        assert!(result.is_err());
    }

    #[test]
    fn secure_open_rejects_symlink_substitution_before_content_open() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root")).unwrap();
        fs::write(temp.path("root/track.wav"), b"synthetic WAV bytes").unwrap();
        fs::write(temp.path("outside.wav"), b"external data").unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();
        let file = walk(&root).unwrap().files.remove(0);
        fs::remove_file(temp.path("root/track.wav")).unwrap();
        symlink(temp.path("outside.wav"), temp.path("root/track.wav")).unwrap();

        let result = with_open_file(&root, &file, |_| ());
        assert!(result.unwrap_err().contains("regular file"));
    }

    #[test]
    fn secure_open_rejects_file_changed_during_parser_callback() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root")).unwrap();
        fs::write(temp.path("root/track.wav"), b"original WAV content").unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();
        let file = walk(&root).unwrap().files.remove(0);
        let path = temp.path("root/track.wav");

        let result = with_open_file(&root, &file, |mut reader| {
            let mut prefix = [0_u8; 1];
            reader.read_exact(&mut prefix).unwrap();
            fs::write(&path, b"replacement content with different size").unwrap();
        });
        assert!(result
            .unwrap_err()
            .contains("changed during metadata extraction"));
    }

    #[test]
    fn bounded_cover_read_accepts_nested_regular_files_and_treats_missing_as_empty() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root/artist/album")).unwrap();
        fs::write(
            temp.path("root/artist/album/cover.jpg"),
            b"\xff\xd8\xffsynthetic",
        )
        .unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();
        assert_eq!(
            read_relative_file(&root, Path::new("artist/album/cover.jpg"), 64).unwrap(),
            Some(b"\xff\xd8\xffsynthetic".to_vec())
        );
        assert_eq!(
            read_relative_file(&root, Path::new("artist/album/missing.png"), 64).unwrap(),
            None
        );
    }

    #[test]
    fn bounded_cover_read_rejects_symlinks_and_oversized_files() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root/album")).unwrap();
        fs::create_dir_all(temp.path("outside")).unwrap();
        fs::write(temp.path("outside/cover.jpg"), b"\xff\xd8\xffexternal").unwrap();
        fs::write(temp.path("root/album/large.jpg"), b"0123456789").unwrap();
        symlink(
            temp.path("outside/cover.jpg"),
            temp.path("root/album/cover.jpg"),
        )
        .unwrap();
        symlink(temp.path("outside"), temp.path("root/linked")).unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();

        assert!(read_relative_file(&root, Path::new("album/cover.jpg"), 64)
            .unwrap_err()
            .contains("symlink"));
        assert!(read_relative_file(&root, Path::new("linked/cover.jpg"), 64)
            .unwrap_err()
            .contains("symlink"));
        assert!(read_relative_file(&root, Path::new("album/large.jpg"), 5)
            .unwrap_err()
            .contains("size limit"));
    }

    #[test]
    fn secure_open_rejects_fifo_without_blocking_on_read_open() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        unsafe extern "C" {
            fn mkfifo(path: *const i8, mode: u32) -> i32;
        }

        let temp = TempDir::new();
        fs::create_dir_all(temp.path("root")).unwrap();
        fs::write(temp.path("root/track.wav"), b"regular file").unwrap();
        let root = fs::canonicalize(temp.path("root")).unwrap();
        let file = walk(&root).unwrap().files.remove(0);
        fs::remove_file(temp.path("root/track.wav")).unwrap();
        let fifo = CString::new(temp.path("root/track.wav").as_os_str().as_bytes()).unwrap();
        let result = unsafe { mkfifo(fifo.as_ptr(), 0o600) };
        assert_eq!(result, 0);

        let result = with_open_file(&root, &file, |_| ());
        assert!(result.unwrap_err().contains("regular file"));
    }
}
