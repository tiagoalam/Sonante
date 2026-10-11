//! Optional sidecar thumbnails, anchored to one published preview generation.
use crate::{audio::LOCAL_FOLDER_COVER_NAMES, local_media::mirror::MirrorManifest};
use base64::{engine::general_purpose::STANDARD, Engine};
use gdk_pixbuf::prelude::*;
use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const IMAGE_LIMIT: u64 = 512 * 1024;

#[derive(Clone)]
pub(crate) struct ArtworkScope(Arc<Anchors>);
impl PartialEq for ArtworkScope {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArtworkScope {}

struct Anchor {
    path: PathBuf,
    file: File,
}
struct Anchors {
    mirror: Anchor,
    sources: BTreeMap<String, Anchor>,
}

fn directory(path: &Path) -> Option<File> {
    if !path.is_absolute() {
        return None;
    }
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open("/")
        .ok()?;
    relative_directory(&root, path.strip_prefix("/").ok()?)
}
fn relative_directory(root: &File, relative: &Path) -> Option<File> {
    let mut file = root.try_clone().ok()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(entry(&file, Path::new(name)))
            .ok()?;
    }
    Some(file)
}
fn entry(parent: &File, name: &Path) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name)
}
fn same(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}
fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    same(a, b)
        && a.mode() == b.mode()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
impl Anchor {
    fn valid(&self) -> Option<()> {
        if same(
            &self.file.metadata().ok()?,
            &directory(&self.path)?.metadata().ok()?,
        ) {
            Some(())
        } else {
            None
        }
    }
}

impl ArtworkScope {
    pub(crate) fn new(path: &Path, manifest: &MirrorManifest, sources: &[String]) -> Option<Self> {
        if sources.is_empty()
            || sources.len() > 8
            || sources.iter().any(|source| source.len() > 256)
        {
            return None;
        }
        let mirror = Anchor {
            path: path.to_path_buf(),
            file: directory(path)?,
        };
        let marker = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(entry(
                &mirror.file,
                Path::new(crate::local_media::mirror::MANIFEST_NAME),
            ))
            .ok()?;
        let metadata = marker.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > 262_144 {
            return None;
        }
        let mut bytes = Vec::new();
        marker.take(262_145).read_to_end(&mut bytes).ok()?;
        let actual: MirrorManifest = serde_json::from_slice(&bytes).ok()?;
        if actual != *manifest {
            return None;
        }
        let mut roots = BTreeMap::new();
        for source in sources {
            if !crate::local_media::is_safe_relative_mpd_uri(source) || source.contains('/') {
                return None;
            }
            let root = manifest.sources.get(source)?;
            roots.insert(
                source.clone(),
                Anchor {
                    path: root.clone(),
                    file: directory(root)?,
                },
            );
        }
        mirror.valid()?;
        Some(Self(Arc::new(Anchors {
            mirror,
            sources: roots,
        })))
    }

    pub(crate) fn published_at(&self, live: &Path, identity: (u64, u64)) -> Option<Self> {
        let metadata = self.0.mirror.file.metadata().ok()?;
        if (metadata.dev(), metadata.ino()) != identity {
            return None;
        }
        let mirror = Anchor {
            path: live.to_path_buf(),
            file: self.0.mirror.file.try_clone().ok()?,
        };
        mirror.valid()?;
        let mut sources = BTreeMap::new();
        for (id, root) in &self.0.sources {
            root.valid()?;
            sources.insert(
                id.clone(),
                Anchor {
                    path: root.path.clone(),
                    file: root.file.try_clone().ok()?,
                },
            );
        }
        Some(Self(Arc::new(Anchors { mirror, sources })))
    }

    pub(crate) fn matches_sources(&self, sources: &[String]) -> bool {
        self.0.sources.len() == sources.len()
            && sources.iter().all(|id| self.0.sources.contains_key(id))
    }

    pub(crate) fn read(&self, folder: &Path) -> Option<String> {
        self.read_with(folder, || {})
    }

    fn read_with(&self, folder: &Path, after_open: impl FnOnce()) -> Option<String> {
        let logical = folder.to_str()?;
        if !crate::local_media::is_safe_relative_mpd_uri(logical) {
            return None;
        }
        let mut parts = folder.components();
        let Component::Normal(source) = parts.next()? else {
            return None;
        };
        let root = self.0.sources.get(source.to_str()?)?;
        let relative = parts.as_path();
        self.0.mirror.valid()?;
        root.valid()?;
        let mirrored = relative_directory(&self.0.mirror.file, folder)?;
        let physical = relative_directory(&root.file, relative)?;
        for name in LOCAL_FOLDER_COVER_NAMES {
            let name = Path::new(name);
            let link_path = entry(&mirrored, name);
            let Ok(link) = fs::symlink_metadata(&link_path) else {
                continue;
            };
            if !link.file_type().is_symlink() {
                return None;
            }
            // Never open the mirror symlink: inspect its exact target and open
            // the expected physical entry relative to the authorized root FD.
            let target = root.path.join(relative).join(name);
            if fs::read_link(&link_path).ok()? != target {
                return None;
            }
            let physical_path = entry(&physical, name);
            let before = fs::symlink_metadata(&physical_path).ok()?;
            if !before.is_file()
                || before.file_type().is_symlink()
                || before.len() == 0
                || before.len() > IMAGE_LIMIT
            {
                return None;
            }
            // O_PATH pins the candidate without opening a possible swapped
            // device/FIFO. Reopen only our verified regular inode via procfs.
            let selected = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
                .open(&physical_path)
                .ok()?;
            if !unchanged(&before, &selected.metadata().ok()?) {
                return None;
            }
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(format!("/proc/self/fd/{}", selected.as_raw_fd()))
                .ok()?;
            if !unchanged(&before, &file.metadata().ok()?) {
                return None;
            }
            after_open();
            let mut bytes = vec![0; before.len() as usize];
            file.read_exact(&mut bytes).ok()?;
            if !unchanged(&before, &file.metadata().ok()?)
                || !unchanged(&before, &fs::symlink_metadata(&physical_path).ok()?)
                || !unchanged(&link, &fs::symlink_metadata(&link_path).ok()?)
                || fs::read_link(&link_path).ok()? != target
                || !same(
                    &mirrored.metadata().ok()?,
                    &relative_directory(&self.0.mirror.file, folder)?
                        .metadata()
                        .ok()?,
                )
                || !same(
                    &physical.metadata().ok()?,
                    &relative_directory(&root.file, relative)?.metadata().ok()?,
                )
            {
                return None;
            }
            self.0.mirror.valid()?;
            root.valid()?;
            return thumbnail(&bytes, name.extension()?.to_str()?);
        }
        None
    }
}

// Inspect dimensions before invoking a decoder; decoding still validates the
// complete image. Reject animation rather than accepting unbounded extra frames.
fn dimensions(bytes: &[u8], kind: &str) -> Option<(u32, u32)> {
    match kind {
        "png" => {
            if bytes.get(12..16)? != b"IHDR" {
                return None;
            }
            let mut at = 8usize;
            while at < bytes.len() {
                let len = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
                let tag = bytes.get(at + 4..at + 8)?;
                if tag == b"acTL" {
                    return None;
                }
                at = at.checked_add(12)?.checked_add(len)?;
                if at > bytes.len() {
                    return None;
                }
                if tag == b"IEND" {
                    break;
                }
            }
            Some((
                u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
                u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
            ))
        }
        "jpeg" => {
            let mut at = 2usize;
            while at < bytes.len() {
                if *bytes.get(at)? != 0xff {
                    return None;
                }
                while *bytes.get(at)? == 0xff {
                    at += 1;
                }
                let marker = *bytes.get(at)?;
                at += 1;
                if marker == 0xda || marker == 0xd9 {
                    return None;
                }
                let len = u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
                if len < 2 {
                    return None;
                }
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    if len < 8 {
                        return None;
                    }
                    return Some((
                        u16::from_be_bytes(bytes.get(at + 5..at + 7)?.try_into().ok()?) as u32,
                        u16::from_be_bytes(bytes.get(at + 3..at + 5)?.try_into().ok()?) as u32,
                    ));
                }
                at = at.checked_add(len)?;
            }
            None
        }
        "webp" => {
            let payload = bytes.get(20..)?;
            let u24 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], 0]);
            match bytes.get(12..16)? {
                b"VP8X" if payload.len() >= 10 && payload[0] & 2 == 0 => {
                    Some((u24(&payload[4..7]) + 1, u24(&payload[7..10]) + 1))
                }
                b"VP8L" if payload.len() >= 5 && payload[0] == 0x2f => {
                    let bits = u32::from_le_bytes(payload[1..5].try_into().ok()?);
                    Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
                }
                b"VP8 " if payload.len() >= 10 && &payload[3..6] == b"\x9d\x01\x2a" => Some((
                    (u16::from_le_bytes(payload[6..8].try_into().ok()?) & 0x3fff) as u32,
                    (u16::from_le_bytes(payload[8..10].try_into().ok()?) & 0x3fff) as u32,
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

fn thumbnail(bytes: &[u8], extension: &str) -> Option<String> {
    let kind = match extension.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        "webp" => "webp",
        _ => return None,
    };
    // Some installed loaders auto-detect despite with_type; require a matching
    // signature as well as a complete successful decode.
    let signature_matches = match kind {
        "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !signature_matches {
        return None;
    }
    let (width, height) = dimensions(bytes, kind)?;
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return None;
    }
    let loader = gdk_pixbuf::PixbufLoader::with_type(kind).ok()?;
    let valid = std::rc::Rc::new(std::cell::Cell::new(false));
    let checked = valid.clone();
    loader.connect_size_prepared(move |loader, w, h| {
        checked.set(w > 0 && h > 0 && w <= 4096 && h <= 4096);
        // Bound the presentation pixels, and reject large declared dimensions.
        let ratio = 256.0 / f64::from(w.max(h).max(1));
        loader.set_size(
            (f64::from(w) * ratio.min(1.0)).max(1.0) as i32,
            (f64::from(h) * ratio.min(1.0)).max(1.0) as i32,
        );
    });
    let written = loader.write(bytes);
    let closed = loader.close();
    if written.is_err() || closed.is_err() || !valid.get() {
        return None;
    }
    let image = loader.pixbuf()?;
    let png = image.save_to_bufferv("png", &[]).ok()?;
    if png.len() as u64 > IMAGE_LIMIT {
        return None;
    }
    Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct Fixture {
        pub root: PathBuf,
        pub mirror: PathBuf,
        pub physical: PathBuf,
        pub manifest: MirrorManifest,
    }
    impl Fixture {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "sonante-preview-art-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let mirror = root.join("library");
            let physical = root.join("physical");
            fs::create_dir(&mirror).unwrap();
            fs::create_dir(&physical).unwrap();
            let manifest = MirrorManifest {
                schema_version: 1,
                generation_id: "preview-test".into(),
                sources: BTreeMap::from([("Nova".into(), physical.clone())]),
            };
            fs::write(
                mirror.join(crate::local_media::mirror::MANIFEST_NAME),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            Self {
                root,
                mirror,
                physical,
                manifest,
            }
        }
        pub(crate) fn sidecar(&self, folder: &str, name: &str, bytes: &[u8]) {
            fs::create_dir_all(self.physical.join(folder)).unwrap();
            fs::create_dir_all(self.mirror.join("Nova").join(folder)).unwrap();
            let target = self.physical.join(folder).join(name);
            fs::write(&target, bytes).unwrap();
            symlink(&target, self.mirror.join("Nova").join(folder).join(name)).unwrap();
        }
        fn scope(&self) -> ArtworkScope {
            ArtworkScope::new(&self.mirror, &self.manifest, &["Nova".into()]).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    pub(crate) fn png() -> Vec<u8> {
        let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 2).unwrap();
        image.fill(0xff0000ff);
        image.save_to_bufferv("png", &[]).unwrap()
    }
    #[test]
    fn preview_artwork_publication_binds_exact_generation_and_prepublication_source_identity() {
        let f = Fixture::new();
        f.sidecar("Album", "cover.png", &png());
        let captured = f.scope();
        let metadata = fs::metadata(&f.mirror).unwrap();
        let identity = (metadata.dev(), metadata.ino());
        let live = f.root.join("published");
        fs::rename(&f.mirror, &live).unwrap();
        assert!(captured
            .published_at(&live, identity)
            .unwrap()
            .read(Path::new("Nova/Album"))
            .is_some());
        assert!(captured
            .published_at(&live, (identity.0, identity.1 + 1))
            .is_none());
        fs::rename(&f.physical, f.root.join("old-source")).unwrap();
        fs::create_dir(&f.physical).unwrap();
        assert!(captured.published_at(&live, identity).is_none());
    }

    #[test]
    fn preview_artwork_oversized_file_is_rejected_before_loading_and_nested_swap_is_rejected() {
        let f = Fixture::new();
        f.sidecar("Album", "cover.png", &vec![0; IMAGE_LIMIT as usize + 1]);
        assert!(f
            .scope()
            .read_with(Path::new("Nova/Album"), || panic!("oversized file opened"))
            .is_none());
        fs::write(f.physical.join("Album/cover.png"), png()).unwrap();
        let scope = f.scope();
        assert!(scope
            .read_with(Path::new("Nova/Album"), || {
                fs::rename(f.physical.join("Album"), f.physical.join("old")).unwrap();
                fs::create_dir(f.physical.join("Album")).unwrap();
                fs::write(f.physical.join("Album/cover.png"), png()).unwrap();
            })
            .is_none());
    }

    #[test]
    fn preview_artwork_rejects_oversized_declared_dimensions_before_decode() {
        let mut bytes = png();
        bytes[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(thumbnail(&bytes, "png").is_none());
        assert!(dimensions(b"\xff\xd8\xff\xc0\x00\x00", "jpeg").is_none());
    }

    #[test]
    fn preview_artwork_decodes_real_jpeg_png_webp_and_reencodes_only_pixels() {
        let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 2, 2).unwrap();
        image.fill(0xff0000ff);
        for (extension, kind) in [("jpg", "jpeg"), ("png", "png")] {
            let bytes = image.save_to_bufferv(kind, &[]).unwrap();
            let uri = thumbnail(&bytes, extension).unwrap();
            assert!(uri.starts_with("data:image/png;base64,"));
            assert!(!uri.contains("physical"));
        }
        let webp = STANDARD.decode("UklGRjwAAABXRUJQVlA4IDAAAADQAQCdASoBAAEAAUAmJaACdLoB+AADsAD+8ut//NgVzXPv9//S4P0uD9Lg/9KQAAA=").unwrap();
        assert!(thumbnail(&webp, "webp").is_some());
        assert!(thumbnail(&png(), "jpg").is_none());
        assert!(thumbnail(b"not an image", "png").is_none());
        assert!(thumbnail(&png()[..24], "png").is_none());
    }
    #[test]
    fn preview_artwork_reads_published_sidecar_and_preserves_filename_priority() {
        let f = Fixture::new();
        f.sidecar("Álbum com espaços/CD1", "cover.png", &png());
        f.sidecar("Álbum com espaços/CD1", "folder.jpg", b"invalid fallback");
        assert!(f
            .scope()
            .read(Path::new("Nova/Álbum com espaços/CD1"))
            .is_some());
        assert!(f.scope().read(Path::new("Nova/Absent")).is_none());
    }
    #[test]
    fn preview_artwork_missing_oversized_invalid_or_nonpublished_file_is_absent() {
        for bytes in [vec![0; IMAGE_LIMIT as usize + 1], b"invalid".to_vec()] {
            let f = Fixture::new();
            f.sidecar("Album", "cover.png", &bytes);
            assert!(f.scope().read(Path::new("Nova/Album")).is_none());
        }
        let f = Fixture::new();
        f.sidecar("Album", "cover.jpg", &png());
        assert!(f.scope().read(Path::new("Nova/Album")).is_none());
        fs::remove_file(f.mirror.join("Nova/Album/cover.jpg")).unwrap();
        assert!(f.scope().read(Path::new("Nova/Album")).is_none());
    }
    #[test]
    fn preview_artwork_rejects_symlink_escape_and_physical_symlinks_without_following_them() {
        let f = Fixture::new();
        f.sidecar("Album", "cover.png", &png());
        let scope = f.scope();
        let link = f.mirror.join("Nova/Album/cover.png");
        let outside = f.root.join("outside.png");
        fs::write(&outside, png()).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&outside, &link).unwrap();
        assert!(scope.read(Path::new("Nova/Album")).is_none());
        fs::remove_file(&link).unwrap();
        symlink(f.physical.join("Album/cover.png"), &link).unwrap();
        fs::remove_file(f.physical.join("Album/cover.png")).unwrap();
        symlink(&outside, f.physical.join("Album/cover.png")).unwrap();
        assert!(scope.read(Path::new("Nova/Album")).is_none());
        for folder in [
            "Outra/Album",
            "../Nova/Album",
            "/Nova/Album",
            "Nova/../Album",
        ] {
            assert!(scope.read(Path::new(folder)).is_none());
        }
    }
    #[test]
    fn preview_artwork_detects_regular_file_or_mirror_link_replacement_during_read() {
        for replace_link in [false, true] {
            let f = Fixture::new();
            f.sidecar("Album", "cover.png", &png());
            let result = f.scope().read_with(Path::new("Nova/Album"), || {
                let path = if replace_link {
                    f.mirror.join("Nova/Album/cover.png")
                } else {
                    f.physical.join("Album/cover.png")
                };
                fs::remove_file(&path).unwrap();
                if replace_link {
                    symlink(f.physical.join("Album/cover.png"), path).unwrap();
                } else {
                    fs::write(path, png()).unwrap();
                }
            });
            assert!(result.is_none());
        }
    }
    #[test]
    fn preview_artwork_rejects_replaced_generation_root_and_nested_directories() {
        for physical in [false, true] {
            let f = Fixture::new();
            f.sidecar("Album", "cover.png", &png());
            let scope = f.scope();
            let path = if physical { &f.physical } else { &f.mirror };
            fs::rename(path, f.root.join("old")).unwrap();
            fs::create_dir(path).unwrap();
            assert!(scope.read(Path::new("Nova/Album")).is_none());
        }
        let f = Fixture::new();
        f.sidecar("Album", "cover.png", &png());
        let scope = f.scope();
        fs::rename(f.mirror.join("Nova/Album"), f.root.join("old")).unwrap();
        symlink(f.root.join("old"), f.mirror.join("Nova/Album")).unwrap();
        assert!(scope.read(Path::new("Nova/Album")).is_none());
    }
}
