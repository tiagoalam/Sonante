use super::*;
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    let end = Instant::now() + Duration::from_secs(5);
    while l.status().unwrap().phase != "ready" {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn indexed(f: &Fixture) -> Library {
    let media = f.0.join("music");
    fs::create_dir(&media).unwrap();
    wave(&media.join("a.wav"), "One", "Own Album");
    wave(&media.join("b.wav"), "Two", "Own Album");
    let l = Library::new(f.0.join("settings.json"));
    l.select(media).unwrap();
    ready(&l);
    l.configure_options(
        None,
        crate::windows_mpd::library::Sources::default(),
        Some(true),
    )
    .unwrap();
    l.complete_first_run().unwrap();
    l
}
fn album(l: &Library) -> String {
    l.albums("", 0, 500).unwrap().items[0].id.clone()
}
#[test]
fn local_sizes_and_titles_are_real_readonly_catalog_observations() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let l = indexed(&f);
    let token = l.metadata_snapshot_token().unwrap();
    let mut tracks = l.metadata_members(&album(&l), &token).unwrap().unwrap();
    tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
    let before = tracks
        .iter()
        .map(|t| fs::read(&t.path).unwrap())
        .collect::<Vec<_>>();
    let p = local(l.clone(), album(&l), l.edition_scope().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        p.sizes,
        Some(before.iter().map(|b| b.len() as u64).collect())
    );
    assert_eq!(p.titles, Some(vec!["One".into(), "Two".into()]));
    for (t, b) in tracks.iter().zip(before) {
        assert_eq!(fs::read(&t.path).unwrap(), b);
    }
    l.stop();
}
#[test]
fn duplicate_and_deleted_members_are_unknown_never_equal() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let l = indexed(&f);
    let token = l.metadata_snapshot_token().unwrap();
    let t = l.metadata_members(&album(&l), &token).unwrap().unwrap();
    let uris = t
        .iter()
        .map(crate::windows_mpd::library::track_uri)
        .collect::<Vec<_>>();
    assert!(crate::windows_file_info::catalog_sizes(
        &l,
        &[uris[0].clone(), uris[0].clone()],
        || true
    )
    .is_err());
    fs::remove_file(&t[1].path).unwrap();
    let p = local(l.clone(), album(&l), l.edition_scope().unwrap())
        .unwrap()
        .unwrap();
    assert!(p.sizes.is_none());
    assert_eq!(p.titles.as_ref().unwrap().len(), 2);
    l.stop();
}
#[test]
fn feature_disable_reenable_settings_and_shutdown_reject_previous_scope() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let l = indexed(&f);
    let id = album(&l);
    let old = l.edition_scope().unwrap();
    l.configure_options(None, old.1, Some(false)).unwrap();
    assert!(l.edition_scope().is_err());
    l.configure_options(None, old.1, Some(true)).unwrap();
    assert_ne!(l.edition_scope().unwrap(), old);
    assert!(local(l.clone(), id, old).is_err());
    assert!(local(
        l.clone(),
        "/own/raw/path".into(),
        l.edition_scope().unwrap()
    )
    .is_err());
    l.stop();
    assert!(l.edition_scope().is_err());
}
#[test]
fn pending_legacy_and_corrupt_preferences_never_enable_or_reset_editions() {
    let _serial = SERIAL.lock().unwrap();
    let f = Fixture::new();
    let path = f.0.join("settings.json");
    let l = Library::new(path.clone());
    l.prepare_first_run(false).unwrap();
    l.configure_options(
        None,
        crate::windows_mpd::library::Sources {
            local: false,
            navidrome: true,
            plex: false,
        },
        Some(true),
    )
    .unwrap();
    assert!(l.edition_scope().is_err());
    l.complete_first_run().unwrap();
    assert!(l.edition_scope().is_ok());
    let epoch = l.status().unwrap().epoch;
    l.configure_options(None, l.sources().unwrap(), Some(false))
        .unwrap();
    assert_eq!(l.status().unwrap().epoch, epoch);
    l.stop();
    let l = Library::new(path.clone());
    assert!(!l.experimental_features().unwrap());
    l.configure_options(None, l.sources().unwrap(), Some(true))
        .unwrap();
    l.stop();
    let l = Library::new(path.clone());
    assert!(l.edition_scope().is_ok());
    fs::write(&path, b"corrupt own fixture").unwrap();
    assert!(l
        .configure_options(None, l.sources().unwrap(), Some(false))
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), b"corrupt own fixture");
    l.stop();
    fs::write(
        &path,
        br#"{"version":3,"first_run":false,"roots":[],"sources":{"local":false,"navidrome":true}}"#,
    )
    .unwrap();
    let bytes = fs::read(&path).unwrap();
    let l = Library::new(path.clone());
    assert!(!l.experimental_features().unwrap());
    assert!(l.edition_scope().is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    l.stop();
}
#[test]
fn physical_permit_remains_owned_until_late_io_and_cleanup_return() {
    let _serial = SERIAL.lock().unwrap();
    // This exercises the real shared worker's publication/retirement contract,
    // without opening a filesystem, engine, listener or user profile.
    let (go, wait) = std::sync::mpsc::channel();
    let (entered, seen) = std::sync::mpsc::channel();
    assert!(ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok());
    let p = Permit;
    let result = crate::windows_io::read(Duration::from_millis(20), move |_| {
        let _p = p;
        entered.send(()).unwrap();
        wait.recv().unwrap();
    });
    assert!(result.is_err());
    seen.recv().unwrap();
    assert!(ACTIVE.load(Ordering::Acquire));
    go.send(()).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while ACTIVE.load(Ordering::Acquire) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
