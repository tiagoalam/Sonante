use super::*;
use std::time::{Duration, Instant};
fn wave(path: &Path, title: &str) {
    let mut body = b"WAVEfmt ".to_vec();
    body.extend(16u32.to_le_bytes());
    body.extend([1, 0, 2, 0]);
    body.extend(44100u32.to_le_bytes());
    body.extend(176400u32.to_le_bytes());
    body.extend([4, 0, 16, 0]);
    body.extend(b"data");
    body.extend(176400u32.to_le_bytes());
    body.extend(vec![0; 176400]);
    let mut info = b"INFO".to_vec();
    for (key, value) in [(b"INAM", title), (b"IPRD", "Álbum"), (b"IART", "Artista")] {
        let mut value = value.as_bytes().to_vec();
        value.push(0);
        info.extend(key);
        info.extend((value.len() as u32).to_le_bytes());
        info.extend(&value);
        if value.len() % 2 != 0 {
            info.push(0);
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
struct Fixture {
    base: PathBuf,
    roots: Vec<PathBuf>,
    library: Library,
}
impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).unwrap();
        let base = std::env::temp_dir().join(format!("sonante-own-source-scope-{random:x?}"));
        let roots = vec![base.join("A 日本/Music"), base.join("B Áudio/Music")];
        for (i, root) in roots.iter().enumerate() {
            fs::create_dir_all(root).unwrap();
            wave(&root.join("one.wav"), &format!("Initial {i}"));
        }
        let library = Library::new(base.join("state/folders.json"));
        library.select_roots(roots.clone()).unwrap();
        ready(&library);
        Self {
            base,
            roots,
            library,
        }
    }
    fn expected(&self) -> Vec<String> {
        self.library
            .configured_roots()
            .unwrap()
            .iter()
            .map(|p| crate::windows_file_info::display_path(p))
            .collect()
    }
    fn check(&self, index: usize) -> Result<bool> {
        self.library.check_changes(
            Some(&source_id(&self.library.configured_roots().unwrap()[index])),
            Some(&self.expected()),
        )
    }
    fn catalog(&self) -> serde_json::Value {
        self.library.compatibility_catalog().unwrap()["albums"].clone()
    }
    fn journal(&self) -> Vec<u8> {
        fs::read(added::path(&self.library.config)).unwrap()
    }
    fn idle_worker(&self) {
        let end = Instant::now() + Duration::from_secs(4);
        while Arc::strong_count(&self.library.data) > 1 {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.library.stop();
        self.idle_worker();
        fs::remove_dir_all(&self.base).unwrap();
    }
}
fn ready(library: &Library) {
    let end = Instant::now() + Duration::from_secs(4);
    while library.status().unwrap().phase == "scanning" {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(library.status().unwrap().phase, "ready");
}
fn models(library: &Library) -> BTreeMap<String, serde_json::Value> {
    library.compatibility_catalog().unwrap()["albums"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["source_id"].as_str().unwrap().into(),
                library
                    .compatibility_tracks(a["id"].as_str().unwrap())
                    .unwrap(),
            )
        })
        .collect()
}
#[test]
fn roots_have_distinct_stable_scopes_and_disambiguated_labels_without_changing_media_uris() {
    let f = Fixture::new();
    let before = f.catalog();
    println!("SCOPE_FIXTURE_JSON:{}", before);
    let ids = before
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["source_id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 2);
    for a in before.as_array().unwrap() {
        assert!(a["source_label"].as_str().unwrap().ends_with("/Music"));
        assert!(a["source_id"]
            .as_str()
            .unwrap()
            .starts_with("windows-root-"));
    }
    let tracks = models(&f.library);
    let configured = f.library.configured_roots().unwrap();
    let mut reversed = configured.clone();
    reversed.reverse();
    f.library.select_roots(reversed).unwrap();
    ready(&f.library);
    assert_eq!(f.catalog(), before);
    assert_eq!(models(&f.library), tracks);
    let reopened = Library::new(f.library.config.clone());
    ready(&reopened);
    assert_eq!(reopened.compatibility_catalog().unwrap()["albums"], before);
    assert_eq!(models(&reopened), tracks);
    reopened.stop();

    assert_eq!(
        f.library.check_changes(None, Some(&f.expected())),
        Ok(false)
    );
    // Stale pre-scope presentation falls back to All in the original UI; the
    // former Windows ID remains valid for old callers requesting an all-root check.
    assert_eq!(
        f.library
            .check_changes(Some("windows-local"), Some(&f.expected())),
        Ok(false)
    );
}
#[test]
fn selected_root_check_discovers_only_that_root_retains_other_models_and_dates_and_all_check_finds_the_rest(
) {
    let f = Fixture::new();
    let old = models(&f.library);
    let config = fs::read(&f.library.config).unwrap();
    let journal: serde_json::Value = serde_json::from_slice(&f.journal()).unwrap();
    let root_b = source_id(&f.library.configured_roots().unwrap()[1]);
    wave(&f.roots[0].join("new.wav"), "New A");
    wave(&f.roots[1].join("new.wav"), "New B");
    wave(&f.roots[1].join("one.wav"), "Changed B");
    assert_eq!(f.check(0), Ok(true));
    assert_eq!(f.check(0), Ok(false));
    let current = models(&f.library);
    assert_eq!(current[&root_b], old[&root_b]);
    assert_eq!(f.library.status().unwrap().tracks, 3);
    assert_eq!(fs::read(&f.library.config).unwrap(), config);
    let next: serde_json::Value = serde_json::from_slice(&f.journal()).unwrap();
    let b_old = old[&root_b][0]["tracks"][0]["path"].as_str().unwrap();
    let b_id = b_old.rsplit('/').next().unwrap();
    assert_eq!(next["tracks"][b_id], journal["tracks"][b_id]);
    assert_eq!(next["tracks"].as_object().unwrap().len(), 3);
    assert_eq!(f.library.check_changes(None, Some(&f.expected())), Ok(true));
    assert_eq!(f.library.status().unwrap().tracks, 4);
    assert_ne!(models(&f.library)[&root_b], old[&root_b]);
    assert_eq!(f.check(0), Ok(false));
}
#[test]
fn selected_removal_and_same_path_tag_update_keep_other_source_and_track_locator() {
    let f = Fixture::new();
    let old = models(&f.library);
    let roots = f.library.configured_roots().unwrap();
    let a = source_id(&roots[0]);
    let b = source_id(&roots[1]);
    let uri = old[&a][0]["tracks"][0]["path"].clone();
    wave(&f.roots[0].join("one.wav"), "Revised A");
    assert_eq!(f.check(0), Ok(true));
    assert_eq!(models(&f.library)[&b], old[&b]);
    assert_eq!(models(&f.library)[&a][0]["tracks"][0]["path"], uri);
    fs::remove_file(f.roots[0].join("one.wav")).unwrap();
    assert_eq!(f.check(0), Ok(true));
    assert_eq!(f.library.status().unwrap().tracks, 1);
    assert!(!models(&f.library).contains_key(&a));
    assert_eq!(models(&f.library)[&b], old[&b]);
    assert_eq!(f.check(0), Ok(false));
}
#[test]
fn unavailable_retained_file_refuses_staged_replacement_and_new_added_events() {
    let f = Fixture::new();
    let before = f.catalog();
    let journal = f.journal();
    let config = fs::read(&f.library.config).unwrap();
    wave(&f.roots[0].join("new.wav"), "Must not publish");
    fs::remove_file(f.roots[1].join("one.wav")).unwrap();
    assert_eq!(f.check(0), Err(OTHER_UNAVAILABLE));
    assert_eq!(f.catalog(), before);
    assert_eq!(f.journal(), journal);
    assert_eq!(fs::read(&f.library.config).unwrap(), config);
    assert_eq!(f.library.status().unwrap().phase, "failed");
}
#[test]
fn unavailable_other_root_refuses_before_epoch_or_catalog_replacement() {
    let f = Fixture::new();
    let before = f.catalog();
    let journal = f.journal();
    let epoch = f.library.status().unwrap().epoch;
    fs::rename(&f.roots[1], f.base.join("moved")).unwrap();
    assert_eq!(f.check(0), Err(OTHER_UNAVAILABLE));
    assert_eq!(f.library.status().unwrap().epoch, epoch);
    assert_eq!(f.catalog(), before);
    assert_eq!(f.journal(), journal);
}
#[test]
fn retained_entry_budget_applies_to_scoped_discovery_and_never_truncates_live_catalog() {
    let f = Fixture::new();
    let before = f.catalog();
    let journal = f.journal();
    {
        let mut state = f.library.data.lock().unwrap();
        let b = state.roots[1].clone();
        state.root_summaries.get_mut(&b).unwrap().entries = 500_000;
        state.signature = signature(&state.roots, &state.root_summaries);
    }
    wave(&f.roots[0].join("new.wav"), "Too many");
    assert_eq!(f.check(0), Err("windows_library_check_unconfirmed"));
    assert_eq!(f.catalog(), before);
    assert_eq!(f.journal(), journal);
}
#[test]
fn superseded_scoped_generation_cannot_publish_catalog_or_added_after_completion() {
    let f = Fixture::new();
    let before = f.catalog();
    let journal = f.journal();
    wave(&f.roots[0].join("new.wav"), "Retired");
    {
        let _change = f.library.change.lock().unwrap();
        let root = f.library.configured_roots().unwrap()[0].clone();
        let seen = f.library.checks.revision().unwrap();
        let started = f.library.begin_source_locked(root).unwrap();
        let end = Instant::now() + Duration::from_secs(4);
        while f.library.checks.revision().unwrap().wrapping_sub(seen) < 2 {
            assert!(
                Instant::now() < end,
                "candidate did not reach private publication"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        f.library.epoch.fetch_add(1, Ordering::AcqRel);
        let mut state = f.library.data.lock().unwrap();
        state.status.epoch = started.epoch + 1;
        state.status.phase = "ready".into();
    }
    f.idle_worker();
    assert_eq!(f.catalog(), before);
    assert_eq!(f.journal(), journal);
}
#[test]
fn scoped_commit_preserves_unrelated_mode_preferences_saved_during_scan() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    let staged = f.library.data.lock().unwrap().clone();
    let candidate = Library {
        data: Arc::new(Mutex::new(staged)),
        ..f.library.clone()
    };
    {
        let mut live = f.library.data.lock().unwrap();
        live.status.phase = "scanning".into();
        live.usage_mode = UsageMode::Simple;
        live.experimental_features = true;
        live.online_artwork_enabled = true;
    }
    f.library.finish_source_scan(&candidate, epoch);
    let live = f.library.data.lock().unwrap();
    assert_eq!(live.status.phase, "ready");
    assert_eq!(live.usage_mode, UsageMode::Simple);
    assert!(live.experimental_features);
    assert!(live.online_artwork_enabled);
}
