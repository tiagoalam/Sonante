use super::{api::Credentials, STORAGE};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u32,
    connection: Option<Credentials>,
}
pub fn load(path: &Path) -> Result<Option<Credentials>, String> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(STORAGE.into()),
    };
    let meta = file.metadata().map_err(|_| STORAGE)?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.len() > 16384
    {
        return Err(STORAGE.into());
    }
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| STORAGE)?;
    if bytes.len() > 16384 {
        return Err(STORAGE.into());
    }
    let store: Store = serde_json::from_slice(&bytes).map_err(|_| STORAGE)?;
    if store.version != 1 {
        return Err(STORAGE.into());
    }
    if let Some(c) = &store.connection {
        c.validate().map_err(|_| STORAGE)?;
    }
    Ok(store.connection)
}
pub fn save(path: &Path, connection: Option<Credentials>) -> Result<(), String> {
    let bytes = serde_json::to_vec(&Store {
        version: 1,
        connection,
    })
    .map_err(|_| STORAGE)?;
    crate::persistence::atomic_write_private(path, &bytes, "navidrome.json")
        .map_err(|_| STORAGE.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn private_store_preserves_other_files_and_rejects_unsafe_inputs() {
        let dir = std::env::temp_dir().join(format!("sonante-nav-store-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("navidrome.json");
        assert!(load(&path).unwrap().is_none());
        let c = Credentials {
            connection_id: "fixture".into(),
            server: "http://localhost:4533/".into(),
            username: "fixture".into(),
            password: "fixture".into(),
        };
        save(&path, Some(c)).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(load(&path).unwrap().is_some());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("missing", &path).unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
