use super::*;
use std::time::{Duration, Instant};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut n = [0; 12];
        getrandom::fill(&mut n).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-roots-{n:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
    fn album(&self, root: &str) -> PathBuf {
        let dir = self.0.join(root);
        fs::create_dir_all(dir.join("Same Album/CD1")).unwrap();
        fs::write(
            dir.join("Same Album/CD1/01 Música.flac"),
            crate::local_file_info::tests::fixture(44100, 16, 2, 44100),
        )
        .unwrap();
        fs::write(dir.join("Same Album/cover.jpg"), b"\xff\xd8\xffsynthetic").unwrap();
        dir
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn ready(library: &Library) {
    let until = Instant::now() + Duration::from_secs(5);
    while library.status().unwrap().phase != "ready" {
        assert!(
            Instant::now() < until,
            "{:?}",
            library.status().unwrap().error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn roots_preserve_opaque_ids_covers_source_labels_and_mixed_selection() {
    let f = Fixture::new();
    let a = f.album("First 音楽");
    let b = f.album("Second música");
    let l = Library::new(f.0.join("folders.json"));
    l.select(a.clone()).unwrap();
    ready(&l);
    let before = l.compatibility_catalog().unwrap()["albums"][0].clone();
    let id = before["id"].as_str().unwrap().to_owned();
    let groups = l.compatibility_tracks(&id).unwrap();
    let cover = l.cover(&id).unwrap();
    l.select_roots(vec![a.clone(), b.clone()]).unwrap();
    ready(&l);
    assert_eq!(l.status().unwrap().tracks, 2);
    let rows = l.compatibility_catalog().unwrap()["albums"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["id"], rows[1]["id"]);
    assert_eq!(rows.iter().find(|r| r["id"] == id).unwrap(), &before);
    assert_eq!(l.compatibility_tracks(&id).unwrap(), groups);
    assert_eq!(l.cover(&id).unwrap(), cover);
    let mut uris = Vec::new();
    for row in &rows {
        let id = row["id"].as_str().unwrap();
        let tracks = l.compatibility_tracks(id).unwrap();
        uris.push(tracks[0]["tracks"][0]["path"].as_str().unwrap().to_owned());
        assert!(l.cover(id).unwrap().is_some());
    }
    let chosen = l.compatibility_selection(&uris, 1).unwrap();
    assert_eq!(chosen.start, 1);
    assert!(chosen.album_id.is_none());
    for track in &chosen.tracks {
        assert!(track.path.starts_with(track.media_root.as_ref().unwrap()));
        let json = serde_json::to_value(track).unwrap();
        assert!(json.get("path").is_none());
        assert!(json.get("media_root").is_none());
    }
    assert_ne!(chosen.tracks[0].media_root, chosen.tracks[1].media_root);
    l.stop();
    let restored = Library::new(f.0.join("folders.json"));
    ready(&restored);
    assert_eq!(restored.configured_roots().unwrap(), vec![a, b]);
    assert_eq!(
        restored.compatibility_catalog().unwrap()["albums"],
        serde_json::json!(rows)
    );
    restored.stop();
}
#[test]
fn invalid_root_sets_and_failed_persistence_keep_previous_configuration_and_catalog() {
    let f = Fixture::new();
    let a = f.album("a");
    let l = Library::new(f.0.join("folders.json"));
    l.select(a.clone()).unwrap();
    ready(&l);
    let before = l.compatibility_catalog().unwrap();
    let saved = fs::read(f.0.join("folders.json")).unwrap();
    for roots in [
        vec![a.clone(), a.join("Same Album")],
        vec![a.clone(), a.clone()],
        vec![f.0.join("missing")],
        vec![a.clone(); 33],
    ] {
        assert!(l.select_roots(roots).is_err());
        assert_eq!(l.compatibility_catalog().unwrap(), before);
        assert_eq!(fs::read(f.0.join("folders.json")).unwrap(), saved);
    }
    assert_eq!(l.configured_roots().unwrap(), vec![a]);
    l.stop();
}
#[test]
fn legacy_migration_is_read_only_and_corrupt_v2_never_falls_back() {
    let f = Fixture::new();
    let a = f.album("a");
    let legacy = f.0.join("legacy.json");
    let next = f.0.join("v2.json");
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"root":a})).unwrap();
    fs::write(&legacy, &bytes).unwrap();
    let l = Library::with_legacy(next.clone(), legacy.clone());
    ready(&l);
    assert_eq!(l.status().unwrap().tracks, 1);
    assert!(!next.exists());
    assert_eq!(fs::read(&legacy).unwrap(), bytes);
    l.stop();
    fs::write(&next, b"corrupt").unwrap();
    let l = Library::with_legacy(next.clone(), legacy.clone());
    assert_eq!(l.status().unwrap().error, Some("library_settings"));
    assert_eq!(fs::read(next).unwrap(), b"corrupt");
    assert_eq!(fs::read(legacy).unwrap(), bytes);
    l.stop();
}
#[test]
fn offline_root_does_not_hide_other_root_and_removal_is_persistent_without_touching_media() {
    let f = Fixture::new();
    let a = f.album("a");
    let b = f.album("b");
    let l = Library::new(f.0.join("folders.json"));
    l.select_roots(vec![a.clone(), b.clone()]).unwrap();
    ready(&l);
    l.stop();
    fs::rename(&b, f.0.join("detached")).unwrap();
    let l = Library::new(f.0.join("folders.json"));
    ready(&l);
    assert_eq!(l.status().unwrap().tracks, 1);
    assert!(l.status().unwrap().warnings > 0);
    assert_eq!(l.configured_roots().unwrap(), vec![a.clone(), b]);
    l.select_roots(vec![a]).unwrap();
    ready(&l);
    assert_eq!(l.status().unwrap().warnings, 0);
    assert!(f.0.join("detached/Same Album/CD1/01 Música.flac").is_file());
    l.select_roots(vec![]).unwrap();
    ready(&l);
    assert!(l.configured_roots().unwrap().is_empty());
    assert_eq!(l.status().unwrap().tracks, 0);
    l.stop();
}

#[cfg(unix)]
#[test]
fn dangling_new_settings_is_an_error_not_permission_to_restore_legacy(){
 let f=Fixture::new();let root=f.album("a");let legacy=f.0.join("legacy.json");let next=f.0.join("v2.json");
 fs::write(&legacy,serde_json::to_vec(&serde_json::json!({"version":1,"root":root})).unwrap()).unwrap();
 std::os::unix::fs::symlink(f.0.join("missing-target"),&next).unwrap();
 let library=Library::with_legacy(next.clone(),legacy);
 assert_eq!(library.status().unwrap().error,Some("library_settings"));assert!(library.configured_roots().unwrap().is_empty());
 assert!(fs::symlink_metadata(next).unwrap().file_type().is_symlink());library.stop();
}
