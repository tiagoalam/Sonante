use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0; 12];
        getrandom::fill(&mut bytes).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-win-catalog-{bytes:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn wave(path: &Path, title: &str, album: &str) {
    wave_tags(path, title, album, "Artista", "")
}
fn wave_tags(path: &Path, title: &str, album: &str, artist: &str, year: &str) {
    let mut body = b"WAVEfmt ".to_vec();
    body.extend(16u32.to_le_bytes());
    body.extend([1, 0, 2, 0]);
    body.extend(44100u32.to_le_bytes());
    body.extend(176400u32.to_le_bytes());
    body.extend([4, 0, 16, 0]);
    body.extend(b"data\0\0\0\0");
    let mut info = b"INFO".to_vec();
    for (name, value) in [
        (b"INAM", title),
        (b"IPRD", album),
        (b"IART", artist),
        (b"ICRD", year),
    ] {
        let mut value = value.as_bytes().to_vec();
        value.push(0);
        info.extend(name);
        info.extend((value.len() as u32).to_le_bytes());
        info.extend(&value);
        if value.len() % 2 != 0 {
            info.push(0)
        }
    }
    body.extend(b"LIST");
    body.extend((info.len() as u32).to_le_bytes());
    body.extend(info);
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    fs::write(path, file).unwrap();
}
fn ready(l: &Library) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = l.status().unwrap();
        if s.phase == "ready" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "scan did not complete: {} {:?}",
            s.phase,
            s.error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn tags_grouping_order_persistence_and_ids() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("CD1")).unwrap();
    fs::create_dir(root.join("CD2")).unwrap();
    wave(&root.join("CD1/01 Música.wav"), "Primeira", "Álbum");
    wave(&root.join("CD2/01 Outra.wav"), "Segunda", "Álbum");
    let l = Library::new(f.0.join("config/folder.json"));
    l.select(root.clone()).unwrap();
    ready(&l);
    let a = l.albums("álbum", 0, 10).unwrap();
    assert_eq!(a.total, 1);
    assert_eq!(a.items[0].track_count, 2);
    let tracks = l.tracks(&a.items[0].id).unwrap();
    assert_eq!(tracks[0].title, "Primeira");
    assert_eq!(tracks[1].disc, 2);
    let selected = l.select_queue(&a.items[0].id, Some(&tracks[1].id)).unwrap();
    assert_eq!(selected.start, 1);
    l.stop();
    let restored = Library::new(f.0.join("config/folder.json"));
    ready(&restored);
    assert_eq!(
        restored.albums("", 0, 10).unwrap().items[0].id,
        a.items[0].id
    );
    restored.stop();
}
#[test]
fn replacement_and_cancel_never_publish_an_old_root() {
    let f = Fixture::new();
    let a = f.0.join("a");
    let b = f.0.join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    for i in 0..100 {
        wave(&a.join(format!("{i}.wav")), "Old", "Old")
    }
    wave(&b.join("new.wav"), "New", "New");
    let l = Library::new(f.0.join("folder.json"));
    l.select(a).unwrap();
    l.select(b).unwrap();
    ready(&l);
    let rows = l.albums("", 0, 10).unwrap();
    assert_eq!(rows.total, 1);
    assert_eq!(rows.items[0].title, "New");
    assert_eq!(l.status().unwrap().tracks, 1);
    l.stop();
}
#[test]
fn invalid_metadata_is_reported_without_losing_file_or_reading_non_images() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("track.flac"), b"invalid").unwrap();
    fs::write(root.join("cover.jpg"), b"not a jpeg").unwrap();
    let l = Library::new(f.0.join("folder.json"));
    l.select(root).unwrap();
    ready(&l);
    assert_eq!(l.status().unwrap().warnings, 1);
    let a = l.albums("", 0, 10).unwrap();
    assert_eq!(a.total, 1);
    assert!(l.cover(&a.items[0].id).unwrap().is_none());
    assert!(l.select_queue("stale-id", None).is_err());
    l.stop();
}
#[cfg(unix)]
#[test]
fn symlinks_and_external_directories_are_not_cataloged() {
    let f = Fixture::new();
    let root = f.0.join("music");
    let outside = f.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    wave(&outside.join("secret.wav"), "Outside", "Outside");
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
    let l = Library::new(f.0.join("folder.json"));
    l.select(root).unwrap();
    ready(&l);
    assert_eq!(l.status().unwrap().tracks, 0);
    l.stop();
}

#[test]
fn missing_saved_folder_can_be_refreshed_after_returning() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    let config = f.0.join("folder.json");
    let l = Library::new(config.clone());
    l.select(root.clone()).unwrap();
    ready(&l);
    l.stop();
    fs::remove_dir(&root).unwrap();
    let restored = Library::new(config);
    assert_eq!(
        restored.status().unwrap().error,
        Some("library_unavailable")
    );
    assert_eq!(
        restored.status().unwrap().root_name.as_deref(),
        Some("music")
    );
    fs::create_dir(&root).unwrap();
    wave(&root.join("return.wav"), "Returned", "Returned");
    restored.refresh().unwrap();
    ready(&restored);
    assert_eq!(restored.status().unwrap().tracks, 1);
    restored.stop();
}
#[test]
fn failed_settings_replacement_preserves_active_catalog() {
    let f = Fixture::new();
    let a = f.0.join("a");
    let b = f.0.join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    wave(&a.join("old.wav"), "Old", "Old");
    wave(&b.join("new.wav"), "New", "New");
    let config = f.0.join("folder.json");
    let l = Library::new(config.clone());
    l.select(a).unwrap();
    ready(&l);
    let epoch = l.status().unwrap().epoch;
    fs::remove_file(&config).unwrap();
    fs::create_dir(&config).unwrap();
    assert!(l.select(b).is_err());
    assert_eq!(l.status().unwrap().epoch, epoch);
    assert_eq!(l.albums("", 0, 10).unwrap().items[0].title, "Old");
    l.stop();
}

#[test]
fn track_and_contributor_search_ignores_case_and_accents() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    wave_tags(
        &root.join("one.wav"),
        "Canção da manhã",
        "Álbum",
        "Participação Especial",
        "2022",
    );
    let l = Library::new(f.0.join("folder.json"));
    l.select(root).unwrap();
    ready(&l);
    for query in [
        "CANCAO",
        "participacao",
        "cancao especial",
        "canc\u{0327}a\u{0303}o",
    ] {
        assert_eq!(l.albums(query, 0, 10).unwrap().total, 1, "{query}");
    }
    assert_eq!(l.albums("cancao inexistente", 0, 10).unwrap().total, 0);
    l.stop();
}
#[test]
fn catalog_sort_is_stable_and_paginates_after_sorting() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    for (folder, title, artist, year) in [
        ("a", "Zênite", "Ana", "2020"),
        ("b", "Água", "Zeca", "2024"),
        ("c", "Brisa", "Bruno", ""),
    ] {
        fs::create_dir(root.join(folder)).unwrap();
        wave_tags(
            &root.join(folder).join("one.wav"),
            "Faixa",
            title,
            artist,
            year,
        );
    }
    let l = Library::new(f.0.join("folder.json"));
    l.select(root).unwrap();
    ready(&l);
    let titles = |order| {
        l.albums_ordered("", 0, 10, order)
            .unwrap()
            .items
            .into_iter()
            .map(|a| a.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(titles("artist"), ["Zênite", "Brisa", "Água"]);
    assert_eq!(titles("title"), ["Água", "Brisa", "Zênite"]);
    assert_eq!(titles("year"), ["Água", "Zênite", "Brisa"]);
    assert_eq!(
        l.albums_ordered("", 1, 1, "year").unwrap().items[0].title,
        "Zênite"
    );
    assert!(l.albums_ordered("", 0, 10, "invalid").is_err());
    l.stop();
}

#[test]
fn shared_ui_models_and_opaque_selection_use_the_same_catalog() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    wave(&root.join("01.wav"), "Canção", "Álbum");
    let l = Library::new(f.0.join("folder.json"));
    l.select(root).unwrap();
    ready(&l);
    let catalog = l.compatibility_catalog().unwrap();
    let a = &catalog["albums"][0];
    assert_eq!(a["source_id"], source_scope::source_id(&l.configured_root().unwrap().unwrap()));
    assert_eq!(a["track_count"], 1);
    let id = a["id"].as_str().unwrap();
    let groups = l.compatibility_tracks(id).unwrap();
    let uri = groups[0]["tracks"][0]["path"].as_str().unwrap().to_owned();
    assert_eq!(
        uri.rsplit_once('/').unwrap().0,
        groups[0]["disc"]["folder_path"].as_str().unwrap()
    );
    assert_eq!(l.compatibility_directory("").unwrap()[0]["path"],"windows");
    assert_eq!(l.compatibility_directory("windows").unwrap()[0]["path"],a["folder_path"]);
    assert_eq!(l.compatibility_directory(a["folder_path"].as_str().unwrap()).unwrap()[0]["path"],uri);
    assert!(l.compatibility_directory("windows/../escape").is_err());
    let selection = l.compatibility_selection(&[uri.clone()], 0).unwrap();
    assert_eq!(selection.tracks[0].title, "Canção");
    assert!(selection.tracks[0].path.exists());
    assert!(serde_json::to_value(&selection.tracks[0]).unwrap().get("path").is_none());
    assert!(l
        .compatibility_selection(&[uri.replace("/1/", "/2/")], 0)
        .is_err());
    assert!(l
        .compatibility_selection(&["../outside.wav".into()], 0)
        .is_err());
    assert!(l.compatibility_selection(&[uri], 1).is_err());
    assert_eq!(
        l.compatibility_search("cancao").unwrap()["tracks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    l.stop();
}

#[test]
fn catalog_duration_is_optional_seconds_and_never_added_to_playback_track() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    let path = root.join("Canção 你好.wav");
    wave(&path, "Primeira", "Álbum");
    let mut bytes = fs::read(&path).unwrap();
    assert_eq!(&bytes[36..40], b"data");
    let count = 176400 * 2;
    bytes[40..44].copy_from_slice(&(count as u32).to_le_bytes());
    bytes.splice(44..44, vec![0; count]);
    let len = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&len.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    let l = Library::new(f.0.join("config/folder.json"));
    l.select(root).unwrap();
    ready(&l);
    let album = l.albums("", 0, 10).unwrap().items.remove(0);
    let tracks = l.compatibility_tracks(&album.id).unwrap();
    assert_eq!(tracks[0]["tracks"][0]["duration"], 2.0);
    let search = l.compatibility_search("primeira").unwrap();
    assert_eq!(search["tracks"][0]["duration"], 2.0);
    let selection = l.select_queue(&album.id, None).unwrap();
    let track = serde_json::to_value(&selection.tracks[0]).unwrap();
    assert!(track.get("duration").is_none());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::remove_file(&path).unwrap();
    assert_eq!(l.compatibility_tracks(&album.id).unwrap(), tracks);
    l.stop();
}

#[test]
fn duration_property_failure_preserves_tags_and_zero_remains_unknown() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    let path = root.join("01 missing-fmt.wav");
    wave(&path, "Valid title", "Album");
    let mut bytes = fs::read(&path).unwrap();
    bytes[12..16].copy_from_slice(b"JUNK");
    fs::write(&path, &bytes).unwrap();
    wave(&root.join("02 zero.wav"), "Zero", "Album");
    let l = Library::new(f.0.join("config/folder.json"));
    l.select(root).unwrap();
    ready(&l);
    let album = l.albums("", 0, 10).unwrap().items.remove(0);
    let tracks = l.compatibility_tracks(&album.id).unwrap();
    assert_eq!(tracks[0]["tracks"][0]["title"], "Valid title");
    assert_eq!(tracks[0]["tracks"][1]["title"], "Zero");
    assert!(tracks[0]["tracks"][0].get("duration").is_none());
    assert!(tracks[0]["tracks"][1].get("duration").is_none());
    assert_eq!(l.status().unwrap().warnings, 0);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    l.stop();
}

#[test]
fn oversized_properties_chunk_refuses_duration_but_preserves_previous_tags() {
    use std::io::{Seek, SeekFrom, Write};
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    let path = root.join("oversized-fmt.wav");
    wave(&path, "Preserved title", "Album");
    let original = fs::read(&path).unwrap();
    let size = 16 * 1024 * 1024 + 2u32;
    let mut header = original[..16].to_vec();
    header.extend(size.to_le_bytes());
    let tail = &original[36..];
    let total = 20 + u64::from(size) + tail.len() as u64;
    header[4..8].copy_from_slice(&((total - 8) as u32).to_le_bytes());
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.seek(SeekFrom::Start(20 + u64::from(size))).unwrap();
    file.write_all(tail).unwrap();
    drop(file);
    let before = fs::metadata(&path).unwrap();
    // Lofty 0.25.4's default per-allocation limit is 16MiB. Do not raise global
    // options to obtain a duration from this synthetic oversized fmt chunk.
    let l = Library::new(f.0.join("config/folder.json"));
    l.select(root).unwrap();
    ready(&l);
    let album = l.albums("", 0, 10).unwrap().items.remove(0);
    let tracks = l.compatibility_tracks(&album.id).unwrap();
    assert_eq!(tracks[0]["tracks"][0]["title"], "Preserved title");
    assert!(tracks[0]["tracks"][0].get("duration").is_none());
    assert_eq!(l.status().unwrap().warnings, 0);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    l.stop();
}
