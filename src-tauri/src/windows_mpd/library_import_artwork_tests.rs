//! Actual protected file-reader/lifecycle tests; injected renderer is explicitly
//! synthetic. These fixtures do NOT execute WIC or prove native Windows decoding.
use super::*;
use base64::Engine;
use std::sync::mpsc;
fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP4sE/uPwAHOgLM1IPMwgAAAABJRU5ErkJggg==").unwrap()
}
fn png_uri() -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png())
    )
}
fn job(f: &Fixture, epoch: u64) -> (String, String) {
    fs::write(f.root.join("cover.png"), png()).unwrap();
    let owner = f.library.imports.admit(true, true).unwrap();
    owner.bind(epoch, &[f.root.clone()]).unwrap();
    f.library
        .epoch
        .store(epoch, std::sync::atomic::Ordering::Release);
    {
        let mut state = f.library.data.lock().unwrap();
        state.status.epoch = epoch;
        state.status.phase = "scanning".into();
        state.roots = vec![f.root.clone()];
        state.root = Some(f.root.clone());
        state.albums.clear();
    }
    let mut r = f.record();
    r.cover = Some(f.root.join("cover.png"));
    let mut rows = vec![r];
    assert!(f.library.publish(epoch, &mut rows, 0, 0, false, false, None));
    let id = accepted(owner.commit().unwrap()).id;
    f.library.imports.inner.lock().unwrap().preview_not_before = None;
    let page = f.library.imports.preview(&id).unwrap().read().unwrap();
    let reference = page.albums[0].artwork_ref.clone().unwrap();
    assert!(reference.starts_with(&format!("cover-v1:{id}:")));
    (id, reference)
}
#[test]
fn issued_reference_reads_actual_bound_sidecar_without_exposing_path_or_media_id() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    let expected = png();
    let actual = f
        .library
        .import_artwork_with(lease, move |bytes| {
            assert_eq!(bytes, expected);
            Ok(Some(png_uri()))
        })
        .unwrap();
    assert_eq!(actual, Some(png_uri()));
    if let Ok(path)=std::env::var("SONANTE_OWN_IMPORT_ARTWORK_JSON") {
        f.library.imports.inner.lock().unwrap().preview_not_before=None;
        let preview=f.library.imports.preview(&id).unwrap().read().unwrap();
        fs::write(path,serde_json::to_vec(&serde_json::json!({"operation":f.library.imports.snapshot().unwrap(),"preview":preview,"image":actual,"synthetic_renderer":true,"native_codec_executed":false})).unwrap()).unwrap();
    }
    assert_eq!(phase(&f.library.imports), Phase::Indexing);
    assert!(!reference.contains(&f.root.to_string_lossy().to_string()));
    assert!(!reference.contains(&f.record().album.id));
}
#[test]
fn malformed_foreign_expired_and_unissued_references_are_rejected_before_any_read() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    for bad in [
        "".into(),
        format!("cover-v1:{id}:../cover.png"),
        format!("cover-v1:{id}:{}", "a".repeat(64)),
        format!("cover-v1:{id}:{}", "G".repeat(64)),
        format!("cover-v1:foreign:{}", "a".repeat(64)),
        "x".repeat(129),
        "https://example.invalid/cover.png".into(),
    ] {
        assert!(f.library.imports.artwork(&id, &bad).is_err());
        assert!(!f.library.imports.inner.lock().unwrap().artwork_busy);
    }
    assert!(f.library.imports.artwork("wrong", &reference).is_err());
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    assert!(matches!(
        f.library.imports.artwork(&id, &reference),
        Err("LOCAL_IMPORT_PREVIEW_BUSY")
    ));
    drop(lease);
    assert!(matches!(
        f.library.imports.artwork(&id, &reference),
        Err("LOCAL_IMPORT_PREVIEW_BUSY")
    ));
}
#[test]
fn invalidating_after_real_read_discards_a_late_synthetic_thumbnail() {
    for stop in [false, true] {
        let f = Fixture::new();
        let (id, reference) = job(&f, 1);
        let lease = f.library.imports.artwork(&id, &reference).unwrap();
        let manager = f.library.imports.clone();
        let lib = f.library.clone();
        let result = f.library.import_artwork_with(lease, move |_| {
            if stop {
                lib.stop()
            } else {
                manager.validating(1)
            };
            Ok(Some(png_uri()))
        });
        assert!(matches!(result, Err("LOCAL_IMPORT_PREVIEW_STALE")));
    }
}
#[test]
fn optional_missing_oversized_or_bad_artwork_never_changes_import_or_bypasses_existing_reader() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    fs::write(f.root.join("cover.png"), vec![0u8; image::LIMIT + 1]).unwrap();
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    assert_eq!(
        f.library
            .import_artwork_with(lease, |_| panic!("oversized image must not reach decoder")),
        Ok(None)
    );
    assert_eq!(phase(&f.library.imports), Phase::Indexing);
    f.library.imports.inner.lock().unwrap().artwork_not_before = None;
    fs::remove_file(f.root.join("cover.png")).unwrap();
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    assert!(f
        .library
        .import_artwork_with(lease, |_| panic!("missing image must not reach decoder"))
        .is_err());
    assert_eq!(phase(&f.library.imports), Phase::Indexing);
    f.library.imports.inner.lock().unwrap().artwork_not_before = None;
    fs::write(f.root.join("cover.png"), b"bad own image").unwrap();
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    assert_eq!(
        f.library.import_artwork_with(lease, |bytes| {
            assert_eq!(image::thumbnail(bytes),Ok(None));
            Ok(None)
        }),
        Ok(None)
    );
    assert_eq!(phase(&f.library.imports), Phase::Indexing);
}
#[test]
fn catalog_or_source_replacement_invalidates_read_authority() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    {
        let mut state = f.library.data.lock().unwrap();
        state.albums.values_mut().next().unwrap().root = f.base.join("other");
    }
    assert!(matches!(
        f.library
            .import_artwork_with(lease, |_| panic!("wrong source cannot reach decoder")),
        Err("LOCAL_IMPORT_PREVIEW_STALE")
    ));
}
#[test]
fn old_physical_decoder_keeps_capacity_across_revocation_and_new_import() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    let (entered, seen) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let library = f.library.clone();
    let reader = std::thread::spawn(move || {
        library.import_artwork_with(lease, move |_| {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Ok(Some(png_uri()))
        })
    });
    seen.recv_timeout(Duration::from_secs(1)).unwrap();
    f.library.imports.finish(1, None);
    let (id2, reference2) = job(&f, 2);
    assert!(matches!(
        f.library.imports.artwork(&id2, &reference2),
        Err("LOCAL_IMPORT_PREVIEW_BUSY")
    ));
    release.send(()).unwrap();
    assert!(matches!(
        reader.join().unwrap(),
        Err("LOCAL_IMPORT_PREVIEW_STALE")
    ));
    f.library.imports.inner.lock().unwrap().artwork_not_before = None;
    let new = f.library.imports.artwork(&id2, &reference2).unwrap();
    assert_eq!(
        f.library.import_artwork_with(new, |_| Ok(Some(png_uri()))),
        Ok(Some(png_uri()))
    );
}
#[test]
fn caller_timeout_does_not_release_artwork_capacity_while_the_real_io_worker_is_still_decoding() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    let (entered, seen) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let library = f.library.clone();
    let caller = std::thread::spawn(move || {
        library.import_artwork_with(lease, move |_| {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Ok(Some(png_uri()))
        })
    });
    seen.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(caller.join().unwrap(), Err("cover_unavailable"));
    assert!(f.library.imports.inner.lock().unwrap().artwork_busy);
    assert!(matches!(
        f.library.imports.artwork(&id, &reference),
        Err("LOCAL_IMPORT_PREVIEW_BUSY")
    ));
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while f.library.imports.inner.lock().unwrap().artwork_busy {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(phase(&f.library.imports), Phase::Indexing);
}
#[cfg(unix)]
#[test]
fn linked_sidecar_is_not_followed_and_ordinary_cover_behavior_is_preserved() {
    let f = Fixture::new();
    let (id, reference) = job(&f, 1);
    let normal = f.library.cover(&f.record().album.id).unwrap();
    assert_eq!(normal, Some(png_uri()));
    let target = f.base.join("outside.png");
    fs::write(&target, b"own outside fixture").unwrap();
    fs::remove_file(f.root.join("cover.png")).unwrap();
    std::os::unix::fs::symlink(&target, f.root.join("cover.png")).unwrap();
    let lease = f.library.imports.artwork(&id, &reference).unwrap();
    assert_eq!(
        f.library
            .import_artwork_with(lease, |_| panic!("symlink must not reach decoder")),
        Ok(None)
    );
    assert_eq!(fs::read(target).unwrap(), b"own outside fixture");
}
