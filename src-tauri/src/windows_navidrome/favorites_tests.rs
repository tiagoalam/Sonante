use super::*;
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sonante-nav-favorite-check-{}",
            api::random().unwrap()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> std::path::PathBuf {
        self.0.join("favorites.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn album() -> Album {
    api::album(
        &serde_json::json!({"id":"same","name":"Fixture","artist":"Artist","coverArt":"cover"}),
    )
    .unwrap()
}
#[test]
fn duplicate_keys_escaped_accounts_and_invalid_records_never_reset_or_overwrite() {
    let f = Fixture::new();
    let path = f.path();
    let key = "a".repeat(64);
    for bytes in [
        b"broken".to_vec(),
        br#"{"version":2,"accounts":{}}"#.to_vec(),
        br#"{"version":1,"version":1,"accounts":{}}"#.to_vec(),
        format!(r#"{{"version":1,"accounts":{{"{key}":[],"{key}":[]}}}}"#).into_bytes(),
        format!(
            r#"{{"version":1,"accounts":{{"{key}":[],"\u0061{}":[]}}}}"#,
            "a".repeat(63)
        )
        .into_bytes(),
        br#"{"version":1,"accounts":{"invalid":[]}}"#.to_vec(),
        br#"{"version":1,"accounts":{}} {}"#.to_vec(),
        vec![b' '; MAX_BYTES as usize + 1],
    ] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(read(&path, &key).is_err());
        assert!(set(&path, &key, "same", Some(album())).is_err());
        assert!(set(&path, &key, "same", None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let row = serde_json::to_value(album()).unwrap();
    let bad = serde_json::json!({"version":1,"accounts":{key.clone():[row.clone(),row]}});
    let bytes = serde_json::to_vec(&bad).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(set(&path, &key, "same", None).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
#[test]
fn favorite_serialization_contains_only_original_album_metadata_and_no_credentials() {
    let f = Fixture::new();
    let path = f.path();
    let c = Credentials {
        connection_id: "PRIVATE-SESSION".into(),
        server: "http://PRIVATE.invalid/".into(),
        username: "PRIVATE-USER".into(),
        password: "PRIVATE-PASSWORD".into(),
    };
    let key = scope(&c);
    set(&path, &key, "same", Some(album())).unwrap();
    let bytes = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
    assert!(!bytes.contains("PRIVATE"));
    let readback = read(&path, &key).unwrap();
    assert_eq!(readback[0].cover_id.as_deref(), Some("cover"));
    let renamed = f.0.join("reopened.json");
    std::fs::rename(&path, &renamed).unwrap();
    assert_eq!(read(&renamed, &key).unwrap()[0].id, "same");
}
#[cfg(unix)]
#[test]
fn linked_and_dangling_store_entries_stay_errors_and_targets_remain_exact() {
    let f = Fixture::new();
    let path = f.path();
    let target = f.0.join("target");
    std::fs::write(&target, b"PRIVATE unrelated").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let key = "a".repeat(64);
    assert!(set(&path, &key, "same", None).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"PRIVATE unrelated");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(read(&path, &key).is_err());
    assert!(set(&path, &key, "same", Some(album())).is_err());
    assert!(!f.0.join("missing").exists());
}
