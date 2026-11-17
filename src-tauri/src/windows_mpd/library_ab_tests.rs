use super::*;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        MutexGuard,
    },
    time::{Duration, Instant},
};
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    library: Library,
    album: String,
    token: String,
    _serial: MutexGuard<'static, ()>,
}
impl Fixture {
    fn new() -> Self {
        let serial = crate::windows_shared::AB_TEST_SERIAL.lock().unwrap();
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).unwrap();
        let base = std::env::temp_dir().join(format!("sonante-own-ab-local-{random:x?}"));
        let root = base.join("music");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("one.wav"), wave(48000, 1)).unwrap();
        fs::write(root.join("two.wav"), wave(44100, 2)).unwrap();
        let library = Library::new(base.join("state/folders.json"));
        library.select_roots(vec![root.clone()]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        library.complete_first_run().unwrap();
        library
            .configure_options(None, Sources::default(), Some(true))
            .unwrap();
        let album = library.albums("", 0, 10).unwrap().items[0].id.clone();
        let token = ab::begin().unwrap();
        Self {
            base,
            root,
            library,
            album,
            token,
            _serial: serial,
        }
    }
    fn policy(&self) -> ab::Check {
        let weak = Arc::downgrade(&ab::job(&self.token).unwrap());
        Arc::new(move || weak.upgrade().is_some_and(|j| j.current()))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        ab::cancel(&self.token);
        self.library.stop();
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn wave(rate: u32, channels: u16) -> Vec<u8> {
    let len = 5 * rate as usize * channels as usize * 2;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + len as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(channels.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * channels as u32 * 2).to_le_bytes());
    bytes.extend((channels * 2).to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((len as u32).to_le_bytes());
    bytes.resize(44 + len, 0);
    bytes
}
fn decode(job: Arc<ab::Job>, side: ab::Side) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match ab::decode(job.clone(), side, 0., 30) {
            Ok(bytes) => return bytes,
            Err(error) => {
                assert_eq!(serde_json::to_value(error).unwrap()["code"],"AB_BUSY","only the original decoder's pre-input-take Busy may be retried in parallel tests");
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
    }
}
#[test]
fn actual_catalog_retained_inputs_and_original_decoder_produce_same_binary_contract_without_player_or_output(
) {
    let f = Fixture::new();
    let inv = f.library.ab_inventory(&f.album).unwrap();
    assert_eq!(inv.rows.len(), 2);
    let job = ab::job(&f.token).unwrap();
    for (side, key, rate, channels) in [
        (ab::Side::A, inv.rows[0].0.key.clone(), 48000, 1),
        (ab::Side::B, inv.rows[1].0.key.clone(), 44100, 2),
    ] {
        let input = f
            .library
            .ab_local_input(&f.album, &key, f.policy())
            .unwrap();
        assert!(input.identity.starts_with("local:"));
        job.install(side, input).unwrap();
        let bytes = decode(job.clone(), side);
        if let Ok(directory) = std::env::var("SONANTE_AB_FIXTURE_OUTPUT") {
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                Path::new(&directory).join(format!("sample-{rate}-{channels}.bin")),
                &bytes,
            )
            .unwrap();
            fs::write(
                Path::new(&directory).join("tracks.json"),
                serde_json::to_vec(&inv.rows.iter().map(|(t, _)| t).collect::<Vec<_>>()).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(&bytes[..4], b"SAB1");
        let n = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(n(4), 48000);
        assert_eq!(n(8), 2);
        assert_eq!(n(12), 240000);
        assert_eq!(n(16), rate);
        assert_eq!(n(20), channels);
        assert_eq!(bytes.len(), 40 + 240000 * 8);
        assert!(bytes[40..].iter().all(|&b| b == 0));
    }
    assert!(job.valid());
    let weak = Arc::downgrade(&job);
    ab::cancel(&f.token);
    drop(job);
    assert!(
        weak.upgrade().is_none(),
        "installed checks must not own their own Job via a strong Arc"
    );
}
#[test]
fn scope_disable_reenable_and_ordinary_catalog_change_never_revive_prepared_input() {
    let f = Fixture::new();
    let inv = f.library.ab_inventory(&f.album).unwrap();
    let input = f
        .library
        .ab_local_input(&f.album, &inv.rows[0].0.key, f.policy())
        .unwrap();
    f.library
        .configure_options(None, Sources::default(), Some(false))
        .unwrap();
    assert!(!(input.alive)());
    f.library
        .configure_options(None, Sources::default(), Some(true))
        .unwrap();
    assert!(!(input.alive)());
    assert!(ab::job(&f.token)
        .unwrap()
        .install(ab::Side::A, input)
        .is_err());
}
#[test]
fn canceled_read_seek_and_unissued_keys_are_refused_without_a_new_path_or_source_lookup() {
    let f = Fixture::new();
    let inv = f.library.ab_inventory(&f.album).unwrap();
    assert!(f
        .library
        .ab_local_input(&f.album, &"a".repeat(64), f.policy())
        .is_err());
    assert!(f.library.ab_inventory("../outside").is_err());
    let live = Arc::new(AtomicBool::new(true));
    let l = live.clone();
    let mut input = f
        .library
        .ab_local_input(
            &f.album,
            &inv.rows[0].0.key,
            Arc::new(move || l.load(Ordering::Acquire)),
        )
        .unwrap();
    let mut first = [0; 4];
    assert_eq!(input.media.read(&mut first).unwrap(), 4);
    assert_eq!(&first, b"RIFF");
    live.store(false, Ordering::Release);
    assert!(input.media.read(&mut first).is_err());
    assert!(input.media.seek(SeekFrom::Start(0)).is_err());
    assert!(!(input.validate)());
}
#[test]
fn simple_mode_beta_and_source_flags_are_native_admission_not_just_hidden_ui() {
    let f = Fixture::new();
    let key = f.library.ab_inventory(&f.album).unwrap().rows[0]
        .0
        .key
        .clone();
    f.library
        .configure_preferences(
            None,
            Sources::default(),
            None,
            None,
            Some(UsageMode::Simple),
        )
        .unwrap();
    assert!(f.library.ab_inventory(&f.album).is_err());
    assert!(f
        .library
        .ab_local_input(&f.album, &key, f.policy())
        .is_err());
    f.library
        .configure_preferences(
            None,
            Sources {
                local: false,
                navidrome: true,
                plex: false,
            },
            Some(true),
            None,
            Some(UsageMode::Full),
        )
        .unwrap();
    assert!(matches!(f.library.ab_inventory(&f.album),Err(error)if error=="AB_SOURCE"));
}
#[cfg(unix)]
#[test]
fn replaced_file_and_link_escape_are_detected_by_retained_physical_validation_before_decode() {
    for linked in [false, true] {
        let f = Fixture::new();
        let inv = f.library.ab_inventory(&f.album).unwrap();
        let track = &inv.rows[0];
        let input = f
            .library
            .ab_local_input(&f.album, &track.0.key, f.policy())
            .unwrap();
        fs::rename(&track.1.path, f.root.join("old.wav")).unwrap();
        if linked {
            let outside = f.base.join("own-outside.wav");
            fs::write(&outside, wave(48000, 1)).unwrap();
            std::os::unix::fs::symlink(outside, &track.1.path).unwrap();
        } else {
            fs::write(&track.1.path, wave(48000, 1)).unwrap();
        }
        assert!(!(input.validate)());
        assert!(ab::job(&f.token)
            .unwrap()
            .install(ab::Side::A, input)
            .is_err());
    }
}
#[test]
fn pure_polling_never_waits_for_catalog_or_configuration_locks() {
    let f = Fixture::new();
    let scope = f.library.edition_scope().unwrap();
    assert!(f.library.ab_scope_current(scope, None));
    let _data = f.library.data.lock().unwrap();
    let before = Instant::now();
    assert!(!f.library.ab_scope_current(scope, None));
    assert!(before.elapsed() < Duration::from_millis(50));
}
