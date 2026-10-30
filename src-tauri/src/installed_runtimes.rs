//! Fixed, package-owned Linux resources. No environment/PATH/resource-directory override.
//! Development overrides remain in their existing debug-only callers.
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::{Path, PathBuf}};

const ROOT: &str = "/usr/lib/sonante/runtimes/v1";
const MANIFEST: &[u8] = include_bytes!("../../packaging/private-runtimes.json");
const LIMIT: u64 = 256 * 1024 * 1024;
const MISSING: &str = "SONANTE_RUNTIME_MISSING";
const UNTRUSTED: &str = "SONANTE_RUNTIME_UNTRUSTED";

#[derive(Clone, Copy)]
pub(crate) enum Kind { Direct, Eq, Camilla }
impl Kind {
    fn name(self) -> &'static str {
        match self { Self::Direct => "mpd-direct", Self::Eq => "mpd-eq", Self::Camilla => "camilladsp" }
    }
}

fn trusted(meta: &fs::Metadata, owner: u32, directory: bool) -> bool {
    meta.uid() == owner && meta.mode() & 0o6022 == 0
        && if directory { meta.is_dir() } else { meta.is_file() && meta.nlink() == 1 }
}

fn open(path: &Path, owner: u32, executable: bool) -> Result<File, String> {
    let file = OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { MISSING } else { UNTRUSTED })?;
    let meta = file.metadata().map_err(|_| UNTRUSTED)?;
    if !trusted(&meta, owner, false) || meta.len() == 0 || meta.len() > LIMIT
        || (executable && meta.mode() & 0o111 == 0) { return Err(UNTRUSTED.into()); }
    Ok(file)
}

fn verify_at(root: &Path, kind: Kind, owner: u32) -> Result<PathBuf, String> {
    let mut manifest = Vec::new();
    open(&root.join("manifest.json"), owner, false)?.take(MANIFEST.len() as u64 + 1)
        .read_to_end(&mut manifest).map_err(|_| UNTRUSTED)?;
    if manifest != MANIFEST { return Err(UNTRUSTED.into()); }
    let parsed: serde_json::Value = serde_json::from_slice(MANIFEST).map_err(|_| UNTRUSTED)?;
    let expected = parsed["binaries"][kind.name()].as_str().ok_or(UNTRUSTED)?;
    let path = root.join(kind.name());
    verify_file(&path, expected, owner, true)?;
    let resources = parsed["resources"][kind.name()].as_object().ok_or(UNTRUSTED)?;
    for (name, value) in resources {
        let relative = Path::new(name);
        if relative.is_absolute() || relative.components().any(|v| !matches!(v, std::path::Component::Normal(_))) {
            return Err(UNTRUSTED.into());
        }
        let file = root.join(relative);
        for parent in file.parent().ok_or(UNTRUSTED)?.ancestors().take_while(|p| p.starts_with(root)) {
            let meta=fs::symlink_metadata(parent).map_err(|_| UNTRUSTED)?;
            if !trusted(&meta, owner, true) { return Err(UNTRUSTED.into()); }
        }
        verify_file(&file, value.as_str().ok_or(UNTRUSTED)?, owner, false)?;
    }
    Ok(path)
}

fn verify_file(path: &Path, expected: &str, owner: u32, executable: bool) -> Result<(), String> {
    let mut file = open(path, owner, executable)?;
    let before = file.metadata().map_err(|_| UNTRUSTED)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0;
    loop {
        let n = file.read(&mut buffer).map_err(|_| UNTRUSTED)?;
        if n == 0 { break; }
        total += n as u64;
        if total > LIMIT { return Err(UNTRUSTED.into()); }
        hash.update(&buffer[..n]);
    }
    let after = file.metadata().map_err(|_| UNTRUSTED)?;
    if total != before.len() || before.len() != after.len()
        || (before.mtime(),before.mtime_nsec(),before.ctime(),before.ctime_nsec())
            != (after.mtime(),after.mtime_nsec(),after.ctime(),after.ctime_nsec())
        || format!("{:x}", hash.finalize()) != expected { return Err(UNTRUSTED.into()); }
    Ok(())
}

pub(crate) fn resolve(kind: Kind) -> Result<PathBuf, String> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return Err(MISSING.into()); }
    let root = Path::new(ROOT);
    // Root-owned immutable ancestry prevents replacement between verification and spawn.
    // A package upgrade can still replace a file: MPD's owned-child hash check remains active.
    for parent in root.ancestors() {
        let meta = fs::symlink_metadata(parent).map_err(|e|
            if e.kind() == std::io::ErrorKind::NotFound { MISSING } else { UNTRUSTED })?;
        if !trusted(&meta, 0, true) { return Err(UNTRUSTED.into()); }
    }
    verify_at(root, kind, 0)
}

pub(crate) fn release(kind: Kind, override_name: &str) -> Result<PathBuf, String> {
    if std::env::var_os(override_name).is_some() {
        return Err("SONANTE_RUNTIME_OVERRIDE_FORBIDDEN".into());
    }
    resolve(kind)
}

pub(crate) fn configure_camilla(command: &mut std::process::Command) {
    if !cfg!(debug_assertions) {
        // Isolate this client DLL from MPD: Shared ALSA must load its host-matched client.
        // Scope only this owned client, never the user's PipeWire daemon/config.
        command.env("LD_LIBRARY_PATH",format!("{ROOT}/lib/camilla"))
            .env("PIPEWIRE_MODULE_DIR",format!("{ROOT}/lib/pipewire-0.3"))
            .env("SPA_PLUGIN_DIR",format!("{ROOT}/lib/spa-0.2"))
            .env("PIPEWIRE_CONFIG_DIR",format!("{ROOT}/share/pipewire"))
            .env_remove("PIPEWIRE_CONFIG_NAME");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[test]
    fn pinned_manifest_and_fixed_location() {
        let m:serde_json::Value=serde_json::from_slice(MANIFEST).unwrap();
        assert_eq!(m["schema"],2); assert_eq!(m["install_root"],ROOT);
        assert_eq!(m["binaries"].as_object().unwrap().len(),3);
        for k in [Kind::Direct,Kind::Eq,Kind::Camilla] { assert_eq!(m["binaries"][k.name()].as_str().unwrap().len(),64); }
    }
    #[test]
    fn rejects_missing_altered_symlink_nonexec_writable_and_wrong_owner() {
        let root=std::env::temp_dir().join(format!("sonante-installed-{}",std::process::id()));
        fs::create_dir(&root).unwrap(); let uid=unsafe {libc::getuid()};
        let manifest=root.join("manifest.json"); let binary=root.join("mpd-direct");
        assert_eq!(verify_at(&root,Kind::Direct,uid).unwrap_err(),MISSING);
        fs::write(&manifest,MANIFEST).unwrap(); fs::set_permissions(&manifest,fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&binary,b"altered").unwrap(); fs::set_permissions(&binary,fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(verify_at(&root,Kind::Direct,uid).unwrap_err(),UNTRUSTED);
        assert!(open(&binary,uid.wrapping_add(1),true).is_err());
        for mode in [0o644,0o777,0o4755,0o2755] { fs::set_permissions(&binary,fs::Permissions::from_mode(mode)).unwrap(); assert!(open(&binary,uid,true).is_err()); }
        fs::remove_file(&binary).unwrap(); symlink(&manifest,&binary).unwrap(); assert!(open(&binary,uid,true).is_err());
        fs::write(&manifest,b"{}").unwrap(); assert_eq!(verify_at(&root,Kind::Direct,uid).unwrap_err(),UNTRUSTED);
        fs::remove_dir_all(root).unwrap();
    }
}
