use super::*;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut r = [0; 12];
        getrandom::fill(&mut r).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-bound-cover-{r:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap()
    }
}
#[test]
fn content_read_uses_exact_held_file_and_preserves_read_errors_without_writes() {
    let f = Fixture::new();
    let root = f.0.join("chosen");
    fs::create_dir(&root).unwrap();
    let path = root.join("cover.jpg");
    let image = b"\xff\xd8\xfffixture";
    fs::write(&path, image).unwrap();
    let bytes = with_bound_file(&root, &path, |mut file, len| {
        assert_eq!(len, image.len() as u64);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        Ok(bytes)
    })
    .unwrap();
    assert_eq!(bytes, image);
    assert_eq!(fs::read(&path).unwrap(), image);
    assert_eq!(
        with_bound_file::<()>(&root, &path, |_, _| Err(InfoError::CorruptHeader)),
        Err(InfoError::CorruptHeader)
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}
#[test]
fn invalid_outside_relative_parent_and_too_deep_paths_never_call_reader() {
    let f = Fixture::new();
    let root = f.0.join("chosen");
    fs::create_dir(&root).unwrap();
    let outside = f.0.join("outside.jpg");
    fs::write(&outside, b"fixture").unwrap();
    for path in [
        outside,
        PathBuf::from("relative.jpg"),
        root.join("../outside.jpg"),
        root.join((0..66).map(|_| "node").collect::<Vec<_>>().join("/")),
    ] {
        assert!(with_bound_file::<()>(&root, &path, |_, _| panic!("no read authority")).is_err());
    }
    assert!(with_bound_file::<()>(&root, &root, |_, _| panic!("not a file")).is_err());
}
#[cfg(not(windows))]
#[test]
fn replaced_cover_or_ancestor_symlink_cannot_read_outside_selected_fixture_root() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let root = f.0.join("chosen");
    fs::create_dir(&root).unwrap();
    let path = root.join("cover.jpg");
    let outside = f.0.join("outside.jpg");
    fs::write(&path, b"inside").unwrap();
    fs::write(&outside, b"outside").unwrap();
    let captured = path.canonicalize().unwrap();
    assert!(captured.starts_with(&root));
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert_eq!(
        File::open(&captured)
            .unwrap()
            .bytes()
            .collect::<io::Result<Vec<_>>>()
            .unwrap(),
        b"outside"
    );
    assert!(with_bound_file::<()>(&root, &captured, |_, _| panic!("no outside bytes")).is_err());
    let elsewhere = f.0.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("cover.jpg"), b"outside").unwrap();
    symlink(&elsewhere, root.join("album")).unwrap();
    assert!(
        with_bound_file::<()>(&root, &root.join("album/cover.jpg"), |_, _| panic!(
            "no ancestor traversal"
        ))
        .is_err()
    );
}
#[cfg(not(windows))]
#[test]
fn portable_replacement_after_capture_reads_only_old_handle_and_refuses_publication() {
    let f = Fixture::new();
    let root = f.0.join("chosen");
    fs::create_dir(&root).unwrap();
    let path = root.join("cover.jpg");
    fs::write(&path, b"inside").unwrap();
    let result = with_bound_file(&root, &path, |mut file, _| {
        fs::rename(&path, root.join("old.jpg")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"inside");
        Ok(bytes)
    });
    assert_eq!(result, Err(InfoError::Stale));
}
#[cfg(not(windows))]
#[test]
fn portable_content_change_during_read_is_not_published() {
    let f = Fixture::new();
    let path = f.0.join("cover.jpg");
    fs::write(&path, b"inside").unwrap();
    let result = with_bound_file(&f.0, &path, |_, _| {
        fs::write(&path, b"changed length").unwrap();
        Ok("must not publish")
    });
    assert_eq!(result, Err(InfoError::Stale));
}
#[cfg(windows)]
#[test]
fn native_held_content_handles_refuse_normal_write_and_delete_sharing() {
    let f = Fixture::new();
    let path = f.0.join("cover.jpg");
    fs::write(&path, b"inside").unwrap();
    with_bound_file(&f.0, &path, |mut file, _| {
        assert!(fs::write(&path, b"change").is_err());
        assert!(fs::rename(&path, f.0.join("moved.jpg")).is_err());
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"inside");
        Ok(())
    })
    .unwrap();
}
