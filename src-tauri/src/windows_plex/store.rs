//! App-local encrypted Windows Plex account; source enablement is separate.
use super::Settings;
const STORAGE: &str = "PLEX_STORAGE";
// Only this internal payload is serialized, inside the DPAPI boundary. The
// source enabled flag and active route/generation are never restored from it.
#[derive(Clone,serde::Serialize,serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Account {
    pub(super) server_url:String,
    pub(super) account_token:String,
    pub(super) server_id:Option<String>,
    pub(super) server_name:Option<String>,
}
impl Account {
    pub(super) fn from_settings(settings:&Settings)->Result<Self,String>{
        let account=Self{server_url:settings.plex_url.clone(),account_token:settings.plex_token.clone(),server_id:settings.plex_server_id.clone(),server_name:settings.plex_server_name.clone()};account.validate()?;Ok(account)
    }
    pub(super) fn settings(&self,enabled:bool)->Settings {
        Settings{enabled,plex_url:self.server_url.clone(),plex_token:self.account_token.clone(),plex_server_id:self.server_id.clone(),plex_server_name:self.server_name.clone()}
    }
    fn validate(&self)->Result<(),String>{
        if self.account_token.is_empty(){return Err(STORAGE.into());}
        self.settings(false).validate().map_err(|_|STORAGE.into())
    }
}

use crate::windows_credentials::{self as secret, Purpose, Sensitive};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};
const MAX_RECORD: usize = 32 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    content: Content,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Content {
    Empty {},
    DpapiUser { data: String },
}
type Open = fn(Purpose, &[u8]) -> secret::Result<Sensitive>;
type Seal = fn(Purpose, &[u8]) -> secret::Result<Vec<u8>>;
fn read(path: &Path, open: Open) -> Result<Option<Account>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
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
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Dangling entries are errors, not absence or permission to overwrite.
            return match std::fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(STORAGE.into()),
            };
        }
        Err(_) => return Err(STORAGE.into()),
    };
    let meta = file.metadata().map_err(|_| STORAGE)?;
    if !meta.is_file() || crate::persistence::linked(&meta) || meta.len() > MAX_RECORD as u64 {
        return Err(STORAGE.into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        };
        if meta.file_attributes()
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            != 0
        {
            return Err(STORAGE.into());
        }
    }
    let mut bytes = Vec::new();
    file.take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| STORAGE)?;
    if bytes.len() > MAX_RECORD {
        return Err(STORAGE.into());
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| STORAGE)?;
    if record.version != 1 {
        return Err(STORAGE.into());
    }
    match record.content {
        Content::Empty {} => Ok(None),
        Content::DpapiUser { data } => {
            if data.is_empty() || data.len() > secret::MAX_SEALED.div_ceil(3) * 4 {
                return Err(STORAGE.into());
            }
            let ciphertext = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| STORAGE)?;
            let plaintext = open(Purpose::Plex, &ciphertext).map_err(|_| STORAGE)?;
            let credentials: Account =
                serde_json::from_slice(plaintext.bytes()).map_err(|_| STORAGE)?;
            credentials.validate().map_err(|_| STORAGE)?;
            Ok(Some(credentials))
        }
    }
}
pub(super) fn load(path: &Path) -> Result<Option<Account>, String> {
    read(path, secret::open)
}
fn write(
    path: &Path,
    connection: Option<Account>,
    seal: Seal,
    open: Open,
) -> Result<(), String> {
    // Explicit connect/disconnect must not replace an unknown/corrupt record.
    drop(read(path, open)?);
    let content = match connection {
        None => Content::Empty {},
        Some(credentials) => {
            credentials.validate().map_err(|_| STORAGE)?;
            let mut plain = Sensitive::new(Vec::with_capacity(secret::MAX_PAYLOAD + 1));
            // Account validation bounds each string before serialization; the reserve
            // avoids growing/reallocating buffers containing secret data.
            serde_json::to_writer(plain.writer(), &credentials).map_err(|_| STORAGE)?;
            if plain.bytes().len() > secret::MAX_PAYLOAD {
                return Err(STORAGE.into());
            }
            let protected = seal(Purpose::Plex, plain.bytes()).map_err(|_| STORAGE)?;
            Content::DpapiUser {
                data: base64::engine::general_purpose::STANDARD.encode(protected),
            }
        }
    };
    let bytes = serde_json::to_vec(&Record {
        version: 1,
        content,
    })
    .map_err(|_| STORAGE)?;
    if bytes.len() > MAX_RECORD {
        return Err(STORAGE.into());
    }
    crate::persistence::atomic_write_private(path, &bytes, "windows-plex-account-v1.json")
        .map_err(|_| STORAGE.into())
}
pub(super) fn save(path: &Path, connection: Option<Account>) -> Result<(), String> {
    write(path, connection, secret::seal, secret::open)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut bytes = [0; 12];
            getrandom::fill(&mut bytes).unwrap();
            let root = std::env::temp_dir().join(format!("sonante-plex-secret-{bytes:x?}"));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("credentials.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn credentials() -> Account {
        Account {server_url:"https://fixture.invalid/prefix/".into(),account_token:"fixture-secret-PRIVATE".into(),server_id:Some("fixture-server".into()),server_name:Some("Fixture".into())}
    }
    fn seal(purpose: Purpose, bytes: &[u8]) -> secret::Result<Vec<u8>> {
        secret::fixture_seal(purpose, bytes)
    }
    fn open(purpose: Purpose, bytes: &[u8]) -> secret::Result<Sensitive> {
        secret::fixture_open(purpose, bytes)
    }
    #[test]
    fn mock_wire_roundtrip_and_explicit_empty_preserve_other_files_without_crypto_claim() {
        let f = Fixture::new();
        let path = f.path();
        let unrelated = f.0.join("unrelated");
        std::fs::write(&unrelated, b"keep").unwrap();
        assert!(read(&path, open).unwrap().is_none());
        write(&path, Some(credentials()), seal, open).unwrap();
        assert_eq!(
            read(&path, open).unwrap().unwrap().account_token,
            "fixture-secret-PRIVATE"
        );
        write(&path, None, seal, open).unwrap();
        assert!(read(&path, open).unwrap().is_none());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
        assert!(path.is_file());
    }
    #[test]
    fn malformed_future_plaintext_missing_fields_and_corruption_never_become_empty_or_overwritten()
    {
        let f = Fixture::new();
        let path = f.path();
        for bytes in [
            b"broken".to_vec(),
            br#"{"version":2,"content":{"kind":"empty"}}"#.to_vec(),
            br#"{"version":1}"#.to_vec(),
            br#"{"version":1,"content":{"kind":"empty","data":"unexpected"}}"#.to_vec(),
            br#"{"version":1,"version":1,"content":{"kind":"empty"}}"#.to_vec(),
            br#"{"version":1,"connection":{"password":"PRIVATE"}}"#.to_vec(),
            br#"{"version":1,"content":{"kind":"dpapi_user","data":"bad"}}"#.to_vec(),
            vec![0; MAX_RECORD + 1],
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(read(&path, open).is_err());
            assert!(write(&path, Some(credentials()), seal, open).is_err());
            assert!(write(&path, None, seal, open).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn provider_failure_never_writes_plaintext_and_prior_valid_record_stays_exact() {
        let f = Fixture::new();
        let path = f.path();
        write(&path, Some(credentials()), seal, open).unwrap();
        let before = std::fs::read(&path).unwrap();
        fn fail(_: Purpose, _: &[u8]) -> secret::Result<Vec<u8>> {
            Err("windows_credentials_unavailable")
        }
        assert!(write(&path, Some(credentials()), fail, open).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[test]
    fn invalid_decrypted_schema_is_storage_error_even_when_protection_is_valid() {
        let f = Fixture::new();
        let path = f.path();
        let data = seal(Purpose::Plex, br#"{"username":"fixture"}"#).unwrap();
        let bytes = serde_json::to_vec(&Record {
            version: 1,
            content: Content::DpapiUser {
                data: base64::engine::general_purpose::STANDARD.encode(data),
            },
        })
        .unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(read(&path, open).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    #[cfg(unix)]
    #[test]
    fn dangling_and_linked_entries_are_errors_and_targets_stay_untouched() {
        let f = Fixture::new();
        let path = f.path();
        let target = f.0.join("target");
        std::fs::write(&target, b"keep").unwrap();
        for entry in [&target, &f.0.join("missing")] {
            std::os::unix::fs::symlink(entry, &path).unwrap();
            assert!(read(&path, open).is_err());
            assert!(write(&path, None, seal, open).is_err());
            std::fs::remove_file(&path).unwrap();
        }
        assert_eq!(std::fs::read(target).unwrap(), b"keep");
    }
    #[cfg(not(windows))]
    #[test]
    fn production_crypto_path_has_no_non_windows_fallback_or_file_creation() {
        let f = Fixture::new();
        let path = f.path();
        assert!(save(&path, Some(credentials())).is_err());
        assert!(!path.exists());
    }
}

#[cfg(test)]
#[path="store_context_tests.rs"]
mod context_tests;
