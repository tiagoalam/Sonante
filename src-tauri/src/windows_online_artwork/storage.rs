//! One private atomic record binds image+metadata. No repair/reset/index cleanup.
use super::{policy, ERROR};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};
const MAX_RECORD: u64 = 12 * 1024 * 1024;
static CACHE: Mutex<()> = Mutex::new(());
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    album_id: String,
    entry: Entry,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    Positive {
        source: String,
        musicbrainz_release_group_id: String,
        mime: String,
        checked_at: u64,
        image: String,
    },
    Negative {
        checked_at: u64,
    },
}
pub(super) enum Hit {
    Image(String),
    Negative,
    Miss,
}
fn filename(id: &str) -> String {
    format!("{:x}.json", Sha256::digest(id.as_bytes()))
}
fn root(path: &Path) -> Result<PathBuf, String> {
    let m = fs::symlink_metadata(path).map_err(|_| ERROR)?;
    if !m.is_dir() || crate::persistence::linked(&m) {
        return Err(ERROR.into());
    }
    let root = fs::canonicalize(path).map_err(|_| ERROR)?;
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        let letter = match root.components().next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(c) | Prefix::VerbatimDisk(c) => c,
                _ => return Err(ERROR.into()),
            },
            _ => return Err(ERROR.into()),
        };
        let drive = [letter as u16, ':' as u16, '\\' as u16, 0];
        if !matches!(unsafe { GetDriveTypeW(drive.as_ptr()) }, 2 | 3) {
            return Err(ERROR.into());
        }
    }
    Ok(root)
}
fn directory(
    root: &Path,
    create: bool,
    current: &dyn Fn() -> bool,
) -> Result<Option<PathBuf>, String> {
    if !current() {
        return Err(ERROR.into());
    }
    let path = root.join("windows-online-artwork-v1");
    if create {
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ERROR.into()),
        }
    }
    match fs::symlink_metadata(&path) {
        Ok(m) if m.is_dir() && !crate::persistence::linked(&m) => Ok(Some(path)),
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Err(ERROR.into()),
    }
}
fn owner() -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::windows_instance::validate().map_err(|_| ERROR)?;
    }
    Ok(())
}
fn bytes(file: &mut File, current: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_file() || crate::persistence::linked(&m) || m.len() > MAX_RECORD {
        return Err(ERROR.into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        };
        if m.file_attributes()
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            != 0
        {
            return Err(ERROR.into());
        }
        super::private_file::verify(file).map_err(|_| ERROR)?;
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        if !current() {
            return Err(ERROR.into());
        }
        let n = file.read(&mut chunk).map_err(|_| ERROR)?;
        if !current() || n as u64 > MAX_RECORD.saturating_sub(bytes.len() as u64) {
            return Err(ERROR.into());
        }
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    Ok(bytes)
}
fn decode(bytes: &[u8], id: &str, now: u64) -> Result<Record, String> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err(ERROR.into());
    }
    let record: Record = serde_json::from_slice(bytes).map_err(|_| ERROR)?;
    if record.version != 1 || record.album_id != id || id.is_empty() || id.len() > 2048 {
        return Err(ERROR.into());
    }
    let checked = match &record.entry {
        Entry::Positive { checked_at, .. } | Entry::Negative { checked_at } => *checked_at,
    };
    if checked > now || checked > 253_402_300_799 {
        return Err(ERROR.into());
    }
    if let Entry::Positive {
        source,
        musicbrainz_release_group_id,
        mime,
        image,
        ..
    } = &record.entry
    {
        if source != "cover_art_archive"
            || !policy::valid_mbid(musicbrainz_release_group_id)
            || image.len() > super::MAX_IMAGE.div_ceil(3) * 4
        {
            return Err(ERROR.into());
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(image)
            .map_err(|_| ERROR)?;
        if decoded.is_empty()
            || decoded.len() > super::MAX_IMAGE
            || policy::image_mime(&decoded) != Some(mime.as_str())
        {
            return Err(ERROR.into());
        }
    }
    Ok(record)
}
fn hit(record: Option<Record>, now: u64) -> Result<Hit, String> {
    match record.map(|r| r.entry) {
        Some(Entry::Negative { checked_at })
            if now.saturating_sub(checked_at) < super::NEGATIVE_TTL =>
        {
            Ok(Hit::Negative)
        }
        Some(Entry::Positive { mime, image, .. }) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(image)
                .map_err(|_| ERROR)?;
            Ok(Hit::Image(policy::data_uri(&mime, &bytes)))
        }
        _ => Ok(Hit::Miss),
    }
}
fn read_record(
    root: &Path,
    dir: &Path,
    id: &str,
    now: u64,
    current: &dyn Fn() -> bool,
) -> Result<Option<Record>, String> {
    let path = dir.join(filename(id));
    match crate::windows_file_info::with_bound_file_while(root, &path, current, |mut f, _| {
        bytes(&mut f, current).map_err(|_| crate::local_file_info::InfoError::Unavailable)
    }) {
        Ok(bytes) => {
            if !current() {
                return Err(ERROR.into());
            }
            Ok(Some(decode(&bytes, id, now)?))
        }
        Err(crate::local_file_info::InfoError::FileNotFound) => match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            _ => Err(ERROR.into()),
        },
        Err(_) => Err(ERROR.into()),
    }
}
pub(super) fn read(
    path: &Path,
    id: &str,
    now: u64,
    current: &dyn Fn() -> bool,
) -> Result<Hit, String> {
    let _lock = CACHE.try_lock().map_err(|_| ERROR)?;
    owner()?;
    let root = root(path)?;
    let Some(dir) = directory(&root, false, current)? else {
        return Ok(Hit::Miss);
    };
    let _held = crate::windows_file_info::ContentDirectory::capture(&root, &dir, current)
        .map_err(|_| ERROR)?;
    let result = hit(read_record(&root, &dir, id, now, current)?, now)?;
    if !current() {
        return Err(ERROR.into());
    }
    Ok(result)
}
fn save(
    path: &Path,
    id: &str,
    entry: Entry,
    now: u64,
    current: &dyn Fn() -> bool,
) -> Result<(), String> {
    let _lock = CACHE.try_lock().map_err(|_| ERROR)?;
    owner()?;
    let root = root(path)?;
    let dir = directory(&root, true, current)?.ok_or(ERROR)?;
    let _held = crate::windows_file_info::ContentDirectory::capture(&root, &dir, current)
        .map_err(|_| ERROR)?;
    // No missing/corrupt/default fallback or automatic repair. Reread even when
    // this request originally observed a miss, before our serialized publication.
    drop(read_record(&root, &dir, id, now, current)?);
    let data = serde_json::to_vec(&Record {
        version: 1,
        album_id: id.into(),
        entry,
    })
    .map_err(|_| ERROR)?;
    decode(&data, id, now)?;
    owner()?;
    if !current() {
        return Err(ERROR.into());
    }
    let path = dir.join(filename(id));
    #[cfg(windows)]
    super::private_file::atomic(&path, &data).map_err(|_| ERROR)?;
    #[cfg(not(windows))]
    crate::persistence::atomic_write_private(&path, &data, "windows-online-artwork-v1")
        .map_err(|_| ERROR)?;
    if !current() {
        return Err(ERROR.into());
    }
    Ok(())
}
pub(super) fn negative(
    path: &Path,
    id: &str,
    now: u64,
    current: &dyn Fn() -> bool,
) -> Result<(), String> {
    save(path, id, Entry::Negative { checked_at: now }, now, current)
}
pub(super) fn positive(
    path: &Path,
    id: &str,
    mbid: &str,
    mime: &str,
    bytes: &[u8],
    now: u64,
    current: &dyn Fn() -> bool,
) -> Result<String, String> {
    if bytes.len() > super::MAX_IMAGE || policy::image_mime(bytes) != Some(mime) {
        return Err(ERROR.into());
    }
    save(
        path,
        id,
        Entry::Positive {
            source: "cover_art_archive".into(),
            musicbrainz_release_group_id: mbid.into(),
            mime: mime.into(),
            checked_at: now,
            image: base64::engine::general_purpose::STANDARD.encode(bytes),
        },
        now,
        current,
    )?;
    Ok(policy::data_uri(mime, bytes))
}
#[cfg(test)]mod tests{
 use super::*;
 const ID:&str="12345678-1234-1234-1234-123456789abc";
 #[test]fn record_schema_binding_time_mime_and_payload_limits_are_fail_closed(){
  let r=Record{version:1,album_id:"own".into(),entry:Entry::Positive{source:"cover_art_archive".into(),musicbrainz_release_group_id:ID.into(),mime:"image/jpeg".into(),checked_at:100,image:base64::engine::general_purpose::STANDARD.encode([255,216,255])}};
  let good=serde_json::to_vec(&r).unwrap();assert!(decode(&good,"own",100).is_ok());assert!(decode(&good,"foreign",100).is_err());assert!(decode(&good,"own",99).is_err());assert!(decode(&vec![0;MAX_RECORD as usize+1],"own",100).is_err());
  for(mode,value)in [("version",serde_json::json!(9)),("image",serde_json::json!("bad")),("mime",serde_json::json!("image/svg+xml")),("musicbrainz_release_group_id",serde_json::json!("../private")),("source",serde_json::json!("other"))]{let mut v=serde_json::to_value(&r).unwrap();if mode=="version"{v[mode]=value}else{v["entry"][mode]=value};assert!(decode(&serde_json::to_vec(&v).unwrap(),"own",100).is_err(),"{mode}");}
 }
 #[test]fn confirmed_negative_ttl_expires_without_deleting_and_positive_bytes_are_exact(){
  let negative=||Record{version:1,album_id:"own".into(),entry:Entry::Negative{checked_at:100}};
  assert!(matches!(hit(Some(negative()),100+super::super::NEGATIVE_TTL-1).unwrap(),Hit::Negative));assert!(matches!(hit(Some(negative()),100+super::super::NEGATIVE_TTL).unwrap(),Hit::Miss));
  assert_eq!(filename("own").len(),69);assert_ne!(filename("own"),filename("foreign"));assert!(!filename("../private").contains("private"));
 }
 #[cfg(not(windows))]struct Fixture(PathBuf);
 #[cfg(not(windows))]impl Fixture{fn new()->Self{let mut n=[0;12];getrandom::fill(&mut n).unwrap();let p=std::env::temp_dir().join(format!("sonante-art-cache-own-{n:x?}"));fs::create_dir(&p).unwrap();Self(p)}fn file(&self,id:&str)->PathBuf{self.0.join("windows-online-artwork-v1").join(filename(id))}}
 #[cfg(not(windows))]impl Drop for Fixture{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
 #[cfg(not(windows))]#[test]fn atomic_image_metadata_roundtrip_and_corruption_never_becomes_negative_or_overwritten(){
  let f=Fixture::new();assert!(matches!(read(&f.0,"own",100,&||true).unwrap(),Hit::Miss));assert!(!f.file("own").exists());let image=[255,216,255,1,2,3];let uri=positive(&f.0,"own",ID,"image/jpeg",&image,100,&||true).unwrap();match read(&f.0,"own",100,&||true).unwrap(){Hit::Image(s)=>assert_eq!(s,uri),_=>panic!("not an image")};let good=fs::read(f.file("own")).unwrap();
  for bad in [Vec::new(),b"broken".to_vec(),br#"{"version":4,"album_id":"own","entry":{"status":"negative","checked_at":100}}"#.to_vec()]{fs::write(f.file("own"),&bad).unwrap();assert!(read(&f.0,"own",100,&||true).is_err());assert!(negative(&f.0,"own",100,&||true).is_err());assert!(positive(&f.0,"own",ID,"image/jpeg",&image,100,&||true).is_err());assert_eq!(fs::read(f.file("own")).unwrap(),bad);}
  fs::write(f.file("own"),&good).unwrap();negative(&f.0,"other",100,&||true).unwrap();assert_eq!(fs::read(f.file("own")).unwrap(),good);assert!(matches!(read(&f.0,"other",100,&||true).unwrap(),Hit::Negative));assert!(negative(&f.0,"cancelled",100,&||false).is_err());assert!(!f.file("cancelled").exists());
 }
 #[cfg(unix)]#[test]fn linked_records_and_directories_are_preserved_never_followed_or_repaired(){
  let f=Fixture::new();let target=f.0.join("target");fs::write(&target,b"preserve own target").unwrap();fs::create_dir(f.0.join("windows-online-artwork-v1")).unwrap();std::os::unix::fs::symlink(&target,f.file("own")).unwrap();assert!(read(&f.0,"own",100,&||true).is_err());assert!(negative(&f.0,"own",100,&||true).is_err());assert_eq!(fs::read(&target).unwrap(),b"preserve own target");
  fs::remove_file(f.file("own")).unwrap();fs::remove_dir(f.0.join("windows-online-artwork-v1")).unwrap();let own=f.0.join("other");fs::create_dir(&own).unwrap();std::os::unix::fs::symlink(&own,f.0.join("windows-online-artwork-v1")).unwrap();assert!(read(&f.0,"own",100,&||true).is_err());assert!(negative(&f.0,"own",100,&||true).is_err());assert!(fs::read_dir(&own).unwrap().next().is_none());
 }
}
