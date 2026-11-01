//! Sonante favorites, isolated by normalized server and username. No passwords.
use super::{api::{self, Album, Credentials}, STORAGE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ALBUMS: usize = 2000;
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u32,
    accounts: BTreeMap<String, Vec<Album>>,
}
pub(super) fn scope(c: &Credentials) -> String {
    let identity = serde_json::to_vec(&[c.server.as_str(), c.username.as_str()]).expect("string array");
    format!("{:x}", Sha256::digest(identity))
}
fn load(path: &Path) -> Result<Store, String> {
    let file = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store { version: 1, ..Store::default() }),
        Err(_) => return Err(STORAGE.into()),
    };
    let m = file.metadata().map_err(|_| STORAGE)?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.nlink() != 1 || m.mode() & 0o077 != 0 || m.len() > MAX_BYTES { return Err(STORAGE.into()); }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| STORAGE)?;
    if bytes.len() as u64 > MAX_BYTES { return Err(STORAGE.into()); }
    let store: Store = crate::source_identity::readonly_unique_json(&bytes).map_err(|_| STORAGE)?;
    if store.version != 1 || store.accounts.len() > 64 || store.accounts.values().map(Vec::len).sum::<usize>() > MAX_ALBUMS { return Err(STORAGE.into()); }
    for (scope, albums) in &store.accounts {
        if scope.len() != 64 || !scope.bytes().all(|b| b.is_ascii_hexdigit()) { return Err(STORAGE.into()); }
        let mut ids = std::collections::BTreeSet::new();
        for a in albums {
            if !ids.insert(&a.id) || a.cover_id.as_deref().is_some_and(|id| !api::id(id)) { return Err(STORAGE.into()); }
            api::album(&serde_json::json!({"id":a.id,"name":a.title,"artist":a.artist,"year":a.year,"coverArt":a.cover_id,"songCount":a.song_count})).map_err(|_| STORAGE)?;
        }
    }
    Ok(store)
}
pub(super) fn read(path: &Path, scope: &str) -> Result<Vec<Album>, String> {
    Ok(load(path)?.accounts.remove(scope).unwrap_or_default())
}
pub(super) fn set(path: &Path, scope: &str, id: &str, album: Option<Album>) -> Result<Vec<Album>, String> {
    let mut store = load(path)?;
    let albums = store.accounts.entry(scope.to_owned()).or_default();
    albums.retain(|a| a.id != id);
    if let Some(album) = album {
        if album.id != id { return Err(STORAGE.into()); }
        albums.insert(0, album);
    }
    let result = albums.clone();
    store.accounts.retain(|_, albums| !albums.is_empty());
    if store.accounts.len() > 64 || store.accounts.values().map(Vec::len).sum::<usize>() > MAX_ALBUMS { return Err(STORAGE.into()); }
    let bytes = serde_json::to_vec(&store).map_err(|_| STORAGE)?;
    if bytes.len() as u64 > MAX_BYTES { return Err(STORAGE.into()); }
    crate::persistence::atomic_write_private_synced(path, &bytes, "navidrome-favorites.json").map_err(|_| STORAGE)?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn account(server: &str, username: &str, connection_id: &str) -> Credentials {
        Credentials { server: server.into(), username: username.into(), connection_id: connection_id.into(), password: "not-saved-in-favorites".into() }
    }
    #[test]
    fn reconnect_keeps_favorites_and_accounts_with_same_album_ids_stay_isolated() {
        let dir = std::env::temp_dir().join(format!("sonante-nav-favorites-{}",std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("navidrome-favorites.json");
        let a = scope(&account("http://fixture/", "one", "old"));
        assert_eq!(a,scope(&account("http://fixture/", "one", "new")));
        let b = scope(&account("http://fixture/", "two", "old"));
        let c = scope(&account("http://other/", "one", "old"));
        assert_ne!(a,b); assert_ne!(a,c);
        let album = api::album(&serde_json::json!({"id":"same-id","name":"One","artist":"Artist"})).unwrap();
        assert!(read(&path,&a).unwrap().is_empty());
        set(&path,&a,&album.id,Some(album.clone())).unwrap();
        set(&path,&b,&album.id,Some(album.clone())).unwrap();
        assert_eq!(read(&path,&a).unwrap().len(),1);
        assert!(read(&path,&c).unwrap().is_empty());
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777,0o600);
        let bytes=std::fs::read(&path).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("not-saved"));
        set(&path,&a,&album.id,None).unwrap();
        assert!(read(&path,&a).unwrap().is_empty());
        assert_eq!(read(&path,&b).unwrap().len(),1);
        let bytes=std::fs::read(&path).unwrap();
        std::fs::write(&path,b"{broken").unwrap();
        assert!(set(&path,&b,&album.id,None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(),b"{broken");
        std::fs::write(&path,bytes).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("missing",&path).unwrap();
        assert!(read(&path,&a).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
