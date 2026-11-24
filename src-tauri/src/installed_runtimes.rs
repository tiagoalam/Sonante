//! Fixed, package-owned Linux resources. No environment/PATH/resource-directory override.
//! Development overrides remain in their existing debug-only callers.
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::{Path, PathBuf}};

const ROOT: &str = "/usr/lib/sonante/runtimes/v1";
const MANIFEST: &[u8] = include_bytes!("../../packaging/private-runtimes.json");
#[cfg(debug_assertions)]
const ARCH_DEV_MANIFEST: &[u8] =
    include_bytes!("../../packaging/qualified-0.5.4/ubuntu-arch.runtime.json");
#[cfg(debug_assertions)]
const FEDORA_DEV_MANIFEST: &[u8] =
    include_bytes!("../../packaging/qualified-0.5.4/fedora.runtime.json");
#[cfg(debug_assertions)]
const ARCH_DEV_MANIFEST_SHA256: &str =
    "ad27855332a39f0433cd038813fd5c71ced64cf22fb293e22dd7f24281d94769";
#[cfg(debug_assertions)]
const FEDORA_DEV_MANIFEST_SHA256: &str =
    "c93f1ac2dc06a8dcee5101858381efc265c3907e50dc2a334a11d293a21cf02f";
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

fn trusted_manifests() -> Result<&'static [&'static [u8]], String> {
    #[cfg(debug_assertions)]
    {
        // Development may use the exact runtime profile installed by the
        // developer's distribution. Pin these extra profiles independently
        // so a changed checkout descriptor cannot silently expand trust.
        if format!("{:x}", Sha256::digest(ARCH_DEV_MANIFEST)) != ARCH_DEV_MANIFEST_SHA256
            || format!("{:x}", Sha256::digest(FEDORA_DEV_MANIFEST)) != FEDORA_DEV_MANIFEST_SHA256
        {
            return Err(UNTRUSTED.into());
        }
        return Ok(&[MANIFEST, ARCH_DEV_MANIFEST, FEDORA_DEV_MANIFEST]);
    }
    #[cfg(not(debug_assertions))]
    Ok(&[MANIFEST])
}

fn verify_at(root: &Path, kind: Kind, owner: u32) -> Result<PathBuf, String> {
    verify_at_with_manifests(root, kind, owner, trusted_manifests()?)
}

fn verify_at_with_manifests(
    root: &Path,
    kind: Kind,
    owner: u32,
    allowed_manifests: &[&[u8]],
) -> Result<PathBuf, String> {
    let max_manifest_size = allowed_manifests
        .iter()
        .map(|manifest| manifest.len())
        .max()
        .ok_or(UNTRUSTED)?;
    let mut manifest = Vec::new();
    open(&root.join("manifest.json"), owner, false)?
        .take(max_manifest_size as u64 + 1)
        .read_to_end(&mut manifest)
        .map_err(|_| UNTRUSTED)?;
    let trusted_manifest = allowed_manifests
        .iter()
        .copied()
        .find(|candidate| *candidate == manifest.as_slice())
        .ok_or(UNTRUSTED)?;
    let parsed: serde_json::Value =
        serde_json::from_slice(trusted_manifest).map_err(|_| UNTRUSTED)?;
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

fn validate_root_ancestry(root: &Path, owner: u32) -> Result<(), String> {
    if !root.is_absolute()
        || root.components().any(|component| {
            matches!(component, std::path::Component::ParentDir | std::path::Component::CurDir)
        })
    {
        return Err(UNTRUSTED.into());
    }
    for parent in root.ancestors() {
        let meta = fs::symlink_metadata(parent)
            .map_err(|error| if error.kind() == std::io::ErrorKind::NotFound {
                MISSING
            } else {
                UNTRUSTED
            })?;
        if !trusted(&meta, owner, true) {
            return Err(UNTRUSTED.into());
        }
    }
    Ok(())
}

fn configured_root(
    experimental_package: bool,
    configured: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if !experimental_package {
        return Ok(PathBuf::from(ROOT));
    }
    let raw = configured.ok_or(MISSING)?;
    let root = PathBuf::from(raw);
    validate_root_ancestry(&root, 0)?;
    Ok(root)
}

pub(crate) fn resolve(kind: Kind) -> Result<PathBuf, String> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return Err(MISSING.into()); }
    let root = configured_root(
        cfg!(feature = "experimental-package"),
        std::env::var_os("SONANTE_EXPERIMENTAL_RUNTIME_ROOT"),
    )?;
    // Root-owned immutable ancestry prevents replacement between verification and spawn.
    // A package upgrade can still replace a file: MPD's owned-child hash check remains active.
    verify_at(&root, kind, 0)
}

pub(crate) fn release(kind: Kind, override_name: &str) -> Result<PathBuf, String> {
    if std::env::var_os(override_name).is_some() {
        return Err("SONANTE_RUNTIME_OVERRIDE_FORBIDDEN".into());
    }
    resolve(kind)
}

pub(crate) fn configure_camilla(command: &mut std::process::Command, runtime_root: &Path) {
    if !cfg!(debug_assertions) {
        // Isolate this client DLL from MPD: Shared ALSA must load its host-matched client.
        // Scope only this owned client, never the user's PipeWire daemon/config.
        command.env("LD_LIBRARY_PATH",runtime_root.join("lib/camilla"))
            .env("PIPEWIRE_MODULE_DIR",runtime_root.join("lib/pipewire-0.3"))
            .env("SPA_PLUGIN_DIR",runtime_root.join("lib/spa-0.2"))
            .env("PIPEWIRE_CONFIG_DIR",runtime_root.join("share/pipewire"))
            .env_remove("PIPEWIRE_CONFIG_NAME");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn test_manifest(binary: &[u8], resources: &[(&str, &[u8])]) -> Vec<u8> {
        let resources = resources
            .iter()
            .map(|(path, contents)| ((*path).to_owned(), serde_json::json!(sha256(contents))))
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "schema": 2,
            "target": "x86_64-unknown-linux-gnu",
            "install_root": ROOT,
            "binaries": { "mpd-direct": sha256(binary) },
            "resources": { "mpd-direct": resources },
        })
        .to_string()
        .into_bytes()
    }

    fn write_test_runtime(
        root: &Path,
        manifest: &[u8],
        binary: &[u8],
        resources: &[(&str, &[u8], u32)],
    ) {
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(root.join("manifest.json"), manifest).unwrap();
        fs::write(root.join("mpd-direct"), binary).unwrap();
        fs::set_permissions(root.join("mpd-direct"), fs::Permissions::from_mode(0o755)).unwrap();
        for (path, contents, mode) in resources {
            let file = root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, contents).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(*mode)).unwrap();
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn development_allowlist_contains_hash_pinned_qualified_arch_and_fedora_profiles() {
        let allowed = trusted_manifests().unwrap();
        assert!(allowed.contains(&MANIFEST));
        assert!(allowed.contains(&ARCH_DEV_MANIFEST));
        assert!(allowed.contains(&FEDORA_DEV_MANIFEST));
        assert_ne!(ARCH_DEV_MANIFEST, FEDORA_DEV_MANIFEST);
        assert_eq!(sha256(ARCH_DEV_MANIFEST), ARCH_DEV_MANIFEST_SHA256);
        assert_eq!(sha256(FEDORA_DEV_MANIFEST), FEDORA_DEV_MANIFEST_SHA256);
        let mut altered_arch_manifest = ARCH_DEV_MANIFEST.to_vec();
        altered_arch_manifest.push(b' ');
        assert!(!allowed.contains(&altered_arch_manifest.as_slice()));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_allowlist_contains_only_the_selected_package_manifest() {
        assert_eq!(trusted_manifests().unwrap(), &[MANIFEST]);
    }

    #[test]
    fn accepts_exact_allowlisted_arch_like_profile_and_verifies_its_extra_resource() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let binary = b"qualified mpd";
        let normal_library = b"qualified codec";
        let nettle = b"qualified nettle";
        let debian = test_manifest(binary, &[("lib/libcodec.so", normal_library)]);
        let arch = test_manifest(
            binary,
            &[
                ("lib/libcodec.so", normal_library),
                ("lib/libnettle.so.8", nettle),
            ],
        );
        assert_ne!(debian, arch);
        write_test_runtime(
            &root,
            &arch,
            binary,
            &[
                ("lib/libcodec.so", normal_library, 0o644),
                ("lib/libnettle.so.8", nettle, 0o644),
            ],
        );

        assert_eq!(
            verify_at_with_manifests(&root, Kind::Direct, unsafe { libc::getuid() }, &[&debian]),
            Err(UNTRUSTED.into()),
            "the default package profile must reject an unlisted Arch manifest"
        );
        assert_eq!(
            verify_at_with_manifests(
                &root,
                Kind::Direct,
                unsafe { libc::getuid() },
                &[&debian, &arch]
            )
            .unwrap(),
            root.join("mpd-direct"),
            "an exact qualified Arch profile is accepted and each listed resource is still hashed"
        );
    }

    #[test]
    fn rejects_hash_mismatch_missing_resource_unsafe_path_and_insecure_permissions() {
        let uid = unsafe { libc::getuid() };
        let binary = b"qualified mpd";
        let good_library = b"qualified library";

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tampered-hash");
        let manifest = test_manifest(binary, &[("lib/libfixture.so", good_library)]);
        write_test_runtime(
            &root,
            &manifest,
            binary,
            &[("lib/libfixture.so", b"tampered", 0o644)],
        );
        assert_eq!(
            verify_at_with_manifests(&root, Kind::Direct, uid, &[&manifest]),
            Err(UNTRUSTED.into())
        );

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("missing-resource");
        let manifest = test_manifest(binary, &[("lib/libfixture.so", good_library)]);
        write_test_runtime(&root, &manifest, binary, &[]);
        assert_eq!(
            verify_at_with_manifests(&root, Kind::Direct, uid, &[&manifest]),
            Err(MISSING.into())
        );

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("unsafe-path");
        let unsafe_manifest = serde_json::json!({
            "binaries": { "mpd-direct": sha256(binary) },
            "resources": { "mpd-direct": { "../outside.so": sha256(good_library) } },
        })
        .to_string()
        .into_bytes();
        write_test_runtime(&root, &unsafe_manifest, binary, &[]);
        assert_eq!(
            verify_at_with_manifests(&root, Kind::Direct, uid, &[&unsafe_manifest]),
            Err(UNTRUSTED.into())
        );

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("unsafe-permissions");
        let manifest = test_manifest(binary, &[("lib/libfixture.so", good_library)]);
        write_test_runtime(
            &root,
            &manifest,
            binary,
            &[("lib/libfixture.so", good_library, 0o666)],
        );
        assert_eq!(
            verify_at_with_manifests(&root, Kind::Direct, uid, &[&manifest]),
            Err(UNTRUSTED.into())
        );
    }

    #[test]
    fn pinned_manifest_and_fixed_location() {
        let m:serde_json::Value=serde_json::from_slice(MANIFEST).unwrap();
        assert_eq!(m["schema"],2); assert_eq!(m["install_root"],ROOT);
        assert_eq!(m["binaries"].as_object().unwrap().len(),3);
        for k in [Kind::Direct,Kind::Eq,Kind::Camilla] { assert_eq!(m["binaries"][k.name()].as_str().unwrap().len(),64); }
    }

    #[test]
    fn experimental_root_is_explicit_and_default_resolution_is_unchanged() {
        let root = PathBuf::from("/usr/lib");
        validate_root_ancestry(&root, 0).unwrap();
        assert_eq!(configured_root(false, None).unwrap(), PathBuf::from(ROOT));
        assert_eq!(
            configured_root(true, Some(root.clone().into_os_string())).unwrap(),
            root
        );
        assert_eq!(configured_root(true, None).unwrap_err(), MISSING);
        assert_eq!(
            configured_root(true, Some("relative-runtime".into())).unwrap_err(),
            UNTRUSTED
        );
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
