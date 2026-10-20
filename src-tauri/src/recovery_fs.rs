//! Anchored, bounded profile-control I/O. Never traverse a music symlink.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};
pub(crate) const UNSAFE: &str = "LIBRARY_RECOVERY_UNSAFE_FILESYSTEM";
pub(crate) const CHANGED: &str = "LIBRARY_RECOVERY_PLAN_CHANGED";
pub(crate) const CANCELLED: &str = "LIBRARY_RECOVERY_CANCELLED";
pub(crate) const INCOMPLETE: &str = "LIBRARY_RECOVERY_SET_INCOMPLETE";
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn check(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub dev: u64,
    pub ino: u64,
}
impl Identity {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
        }
    }
}
pub(crate) struct Dir {
    pub path: PathBuf,
    pub file: File,
    id: Identity,
}
impl Dir {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| UNSAFE)?;
        let m = file.metadata().map_err(|_| UNSAFE)?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
            return Err(UNSAFE.into());
        }
        let result = Self {
            path: path.into(),
            file,
            id: Identity::of(&m),
        };
        result.validate()?;
        Ok(result)
    }
    pub fn id(&self) -> Identity {
        self.id.clone()
    }
    pub fn file_identity(&self, name: &str) -> Result<Option<Identity>, String> {
        if !self.exists(name)? {
            return Ok(None);
        }
        let m = fs::symlink_metadata(self.entry(name)?).map_err(|_| CHANGED)?;
        if !m.is_file() || m.nlink() != 1 || m.uid() != unsafe { libc::geteuid() } {
            return Err(UNSAFE.into());
        }
        Ok(Some(Identity::of(&m)))
    }
    pub fn validate(&self) -> Result<(), String> {
        let m = fs::symlink_metadata(&self.path).map_err(|_| CHANGED)?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o022 != 0
            || Identity::of(&m) != self.id
        {
            return Err(CHANGED.into());
        }
        Ok(())
    }
    pub fn entry(&self, name: &str) -> Result<PathBuf, String> {
        if name.is_empty()
            || Path::new(name)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || name.contains('\0')
        {
            return Err(UNSAFE.into());
        }
        Ok(PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd())).join(name))
    }
    pub fn sync(&self) -> Result<(), String> {
        self.validate()?;
        self.file.sync_all().map_err(|_| INCOMPLETE)?;
        self.validate()
    }
    pub fn child(&self, name: &str) -> Result<Dir, String> {
        Self::open(&self.entry(name)?)
    }
    pub fn create(&self, name: &str) -> Result<Dir, String> {
        self.validate()?;
        let path = self.entry(name)?;
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|_| INCOMPLETE)?;
        self.sync()?;
        self.child(name)
    }
    pub fn exists(&self, name: &str) -> Result<bool, String> {
        match fs::symlink_metadata(self.entry(name)?) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(UNSAFE.into()),
        }
    }
    pub fn read(
        &self,
        name: &str,
        limit: u64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<u8>>, String> {
        if !self.exists(name)? {
            return Ok(None);
        }
        self.validate()?;
        let path = self.entry(name)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| UNSAFE)?;
        let before = file.metadata().map_err(|_| UNSAFE)?;
        if !before.is_file()
            || before.uid() != unsafe { libc::geteuid() }
            || before.nlink() != 1
            || (if matches!(
                name,
                "mpd-v2.db"
                    | "mpd.conf"
                    | ".sonante-mirror.json"
                    | "mpd-v2.db.writing"
                    | "retained-control-mpd-v2.db"
            ) {
                before.mode() & 0o022 != 0
            } else {
                before.mode() & 0o077 != 0
            })
            || before.len() > limit
        {
            return Err(UNSAFE.into());
        }
        let mut data = Vec::new();
        let mut chunk = [0u8; 65536];
        loop {
            check(cancelled)?;
            let n = file.read(&mut chunk).map_err(|_| INCOMPLETE)?;
            if n == 0 {
                break;
            }
            if data.len() as u64 + n as u64 > limit {
                return Err(INCOMPLETE.into());
            }
            data.extend_from_slice(&chunk[..n]);
        }
        let after = file.metadata().map_err(|_| CHANGED)?;
        let current = fs::symlink_metadata(&path).map_err(|_| CHANGED)?;
        if Identity::of(&before) != Identity::of(&current)
            || stamp(&before) != stamp(&after)
            || !current.is_file()
        {
            return Err(CHANGED.into());
        }
        self.validate()?;
        Ok(Some(data))
    }
    pub fn write_new(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.write_new_tracked(name, data, &|_| {})
    }
    pub fn write_new_tracked(
        &self,
        name: &str,
        data: &[u8],
        created: &dyn Fn(Identity),
    ) -> Result<(), String> {
        self.validate()?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.entry(name)?)
            .map_err(|_| INCOMPLETE)?;
        created(Identity::of(&f.metadata().map_err(|_| INCOMPLETE)?));
        f.write_all(data)
            .and_then(|_| f.sync_all())
            .map_err(|_| INCOMPLETE)?;
        self.sync()
    }
    pub fn replace(
        &self,
        name: &str,
        data: &[u8],
        expected_before: Option<&Identity>,
    ) -> Result<(), String> {
        let temp = format!("{name}.writing");
        if self.read(&temp, 256 * 1024 * 1024, &|| false)?.as_deref() != Some(data) {
            return Err(CHANGED.into());
        }
        if let Some(expected) = expected_before {
            if self.file_identity(name)?.as_ref() != Some(expected) {
                return Err(CHANGED.into());
            }
            crate::local_media::mirror::atomic_exchange(&self.entry(name)?, &self.entry(&temp)?)?;
            self.sync()?;
            // If a noncooperating writer won the final pathname race, its actual file is
            // retained in .writing. Refuse completion; never silently discard its bytes.
            if self.file_identity(&temp)?.as_ref() != Some(expected) {
                return Err(CHANGED.into());
            }
        } else {
            self.rename(&temp, self, name, true)?;
        }
        self.sync()
    }

    pub fn rename(&self, from: &str, to: &Dir, name: &str, no_replace: bool) -> Result<(), String> {
        self.validate()?;
        to.validate()?;
        let a = std::ffi::CString::new(from).map_err(|_| UNSAFE)?;
        let b = std::ffi::CString::new(name).map_err(|_| UNSAFE)?;
        self.entry(from)?;
        to.entry(name)?;
        let rc = unsafe {
            libc::renameat2(
                self.file.as_raw_fd(),
                a.as_ptr(),
                to.file.as_raw_fd(),
                b.as_ptr(),
                if no_replace {
                    libc::RENAME_NOREPLACE
                } else {
                    0
                },
            )
        };
        if rc != 0 {
            return Err(CHANGED.into());
        }
        self.sync()?;
        to.sync()
    }
}
fn stamp(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.mode(),
    )
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TreeEntry {
    Directory,
    Link { target: PathBuf },
    Manifest { bytes: Vec<u8> },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tree {
    pub identity: Identity,
    pub entries: BTreeMap<PathBuf, TreeEntry>,
}
impl Tree {
    pub fn capture(
        root: &Dir,
        name: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, String> {
        if !root.exists(name)? {
            return Ok(None);
        }
        let dir = root.child(name)?;
        let mut entries = BTreeMap::new();
        Self::walk(&dir, Path::new(""), &mut entries, cancelled, 0)?;
        dir.validate()?;
        Ok(Some(Self {
            identity: dir.id(),
            entries,
        }))
    }
    fn walk(
        dir: &Dir,
        prefix: &Path,
        out: &mut BTreeMap<PathBuf, TreeEntry>,
        cancelled: &dyn Fn() -> bool,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 128 || out.len() > 262144 {
            return Err(INCOMPLETE.into());
        }
        for item in fs::read_dir(PathBuf::from(format!(
            "/proc/self/fd/{}",
            dir.file.as_raw_fd()
        )))
        .map_err(|_| UNSAFE)?
        {
            check(cancelled)?;
            let item = item.map_err(|_| UNSAFE)?;
            let name = item.file_name();
            let text = name.to_str().ok_or(UNSAFE)?;
            let rel = prefix.join(&name);
            if rel.as_os_str().len() > 4096 {
                return Err(INCOMPLETE.into());
            }
            let p = dir.entry(text)?;
            let m = fs::symlink_metadata(&p).map_err(|_| CHANGED)?;
            let value = if m.is_dir() {
                let child = dir.child(text)?;
                Self::walk(&child, &rel, out, cancelled, depth + 1)?;
                TreeEntry::Directory
            } else if m.file_type().is_symlink() {
                TreeEntry::Link {
                    target: fs::read_link(&p).map_err(|_| CHANGED)?,
                }
            } else if text == crate::local_media::mirror::MANIFEST_NAME && m.is_file() {
                // Existing manifests may be0644; copy their exact bytes after anchored
                // identity/type/UID/nlink checks without following physical sources.
                let mut f = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&p)
                    .map_err(|_| UNSAFE)?;
                let fm = f.metadata().map_err(|_| UNSAFE)?;
                if fm.nlink() != 1
                    || fm.uid() != unsafe { libc::geteuid() }
                    || fm.len() > 262144
                    || Identity::of(&fm) != Identity::of(&m)
                {
                    return Err(UNSAFE.into());
                }
                let mut bytes = Vec::new();
                (&mut f)
                    .take(262145)
                    .read_to_end(&mut bytes)
                    .map_err(|_| UNSAFE)?;
                if bytes.len() > 262144 || stamp(&fm) != stamp(&f.metadata().map_err(|_| CHANGED)?)
                {
                    return Err(CHANGED.into());
                }
                TreeEntry::Manifest { bytes }
            } else {
                return Err(UNSAFE.into());
            };
            if out.insert(rel, value).is_some() {
                return Err(UNSAFE.into());
            }
        }
        dir.validate()
    }
}
