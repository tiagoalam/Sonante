use super::*;
fn original_external_cover(path: &Path, root: &Path) -> Result<Option<String>> {
    let meta = fs::symlink_metadata(path).map_err(|_| "cover_unavailable")?;
    if linked(&meta) || !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
        return Ok(None);
    }
    let path = path.canonicalize().map_err(|_| "cover_unavailable")?;
    if !path.starts_with(root) {
        return Err("library_changed");
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "cover_unavailable")?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cover_unavailable")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Ok(None);
    }
    Ok(image_uri(&bytes))
}
fn original_embedded_cover(path: &Path, root: &Path) -> Result<Option<String>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "cover_unavailable")?;
    if linked(&metadata) || !metadata.is_file() {
        return Ok(None);
    }
    let path = path.canonicalize().map_err(|_| "cover_unavailable")?;
    if !path.starts_with(root) {
        return Err("library_changed");
    }
    let file = File::open(path).map_err(|_| "cover_unavailable")?;
    let tagged = Probe::new(Budget {
        file,
        remaining: 8 * 1024 * 1024,
    })
    .options(
        ParseOptions::new()
            .read_properties(false)
            .read_cover_art(true),
    )
    .guess_file_type()
    .ok()
    .and_then(|p| p.read().ok());
    Ok(tagged.as_ref().and_then(|f| {
        f.tags()
            .iter()
            .flat_map(|t| t.pictures())
            .find_map(|picture| image_uri(picture.data()))
    }))
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut r = [0; 12];
        getrandom::fill(&mut r).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-cover-catalog-{r:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap()
    }
}
fn flac() -> Vec<u8> {
    crate::local_file_info::tests::fixture(44100, 16, 2, 44100)
}
fn embedded(image: &[u8]) -> Vec<u8> {
    let mut b = flac();
    b[4] &= 0x7f;
    let mut picture = Vec::new();
    picture.extend(3u32.to_be_bytes());
    picture.extend(10u32.to_be_bytes());
    picture.extend(b"image/jpeg");
    picture.extend(0u32.to_be_bytes());
    for n in [1u32, 1, 24, 0] {
        picture.extend(n.to_be_bytes())
    }
    picture.extend((image.len() as u32).to_be_bytes());
    picture.extend(image);
    b.push(0x86);
    b.extend(&(picture.len() as u32).to_be_bytes()[1..]);
    b.extend(picture);
    b
}
fn ready(l: &Library) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let s = l.status().unwrap();
        if s.phase == "ready" {
            return;
        }
        assert_ne!(s.phase, "failed");
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2))
    }
}
#[test]
fn external_mime_size_and_invalid_behavior_match_exact_former_read_sequence() {
    let f = Fixture::new();
    let path = f.0.join("cover.jpg");
    for image in [
        b"\xff\xd8\xfffixture".to_vec(),
        b"\x89PNG\r\n\x1a\nfixture".to_vec(),
        b"invalid".to_vec(),
        vec![1; 4 * 1024 * 1024 + 1],
    ] {
        fs::write(&path, &image).unwrap();
        assert_eq!(
            external_cover(&path, &f.0),
            original_external_cover(&path, &f.0)
        );
    }
    assert_eq!(external_cover(&path, &f.0), Ok(None));
}
#[test]
fn embedded_flac_cover_and_missing_art_match_original_parser_and_budget() {
    let f = Fixture::new();
    let path = f.0.join("fixture.flac");
    let image = b"\xff\xd8\xfffixture";
    for bytes in [embedded(image), flac(), b"invalid audio".to_vec()] {
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            embedded_cover(&path, &f.0),
            original_embedded_cover(&path, &f.0)
        );
    }
    fs::write(&path, embedded(image)).unwrap();
    assert_eq!(embedded_cover(&path, &f.0), Ok(image_uri(image)));
}
#[test]
fn catalog_uses_external_priority_then_embedded_without_starting_an_engine() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    let image = b"\xff\xd8\xffembedded";
    fs::write(root.join("fixture.flac"), embedded(image)).unwrap();
    let external = b"\x89PNG\r\n\x1a\nexternal";
    fs::write(root.join("cover.png"), external).unwrap();
    let l = Library::new(f.0.join("folder.json"));
    l.select(root.clone()).unwrap();
    ready(&l);
    let id = l.albums("", 0, 10).unwrap().items[0].id.clone();
    assert_eq!(l.cover(&id), Ok(image_uri(external)));
    fs::remove_file(root.join("cover.png")).unwrap();
    l.refresh().unwrap();
    ready(&l);
    assert_eq!(l.cover(&id), Ok(image_uri(image)));
    l.stop();
    assert_eq!(l.cover(&id), Err("library_changed"));
}
#[test]
fn selected_cover_context_refuses_root_epoch_or_member_changes_without_publishing_data() {
    let f = Fixture::new();
    let root = f.0.join("music");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("fixture.flac"), flac()).unwrap();
    let l = Library::new(f.0.join("folder.json"));
    l.select(root.clone()).unwrap();
    ready(&l);
    let id = l.albums("", 0, 10).unwrap().items[0].id.clone();
    let selected = CoverSource::capture(&l, &id).unwrap();
    assert_eq!(selected.validate(&l), Ok(()));
    {
        let mut state = l.data.lock().unwrap();
        state.albums.get_mut(&id).unwrap().cover = Some(root.join("new.png"));
    }
    assert_eq!(selected.validate(&l), Err("library_changed"));
    let next = f.0.join("next");
    fs::create_dir(&next).unwrap();
    fs::write(next.join("other.flac"), flac()).unwrap();
    l.select(next).unwrap();
    assert_eq!(selected.validate(&l), Err("library_changed"));
    ready(&l);
    l.stop();
}
