//! Explicit, read-only metadata inspection using the existing catalog/handle boundary.
mod catalog;
pub(crate) mod doctor;
pub(crate) mod types;
use crate::{local_file_info::InfoError, windows_mpd::library::Library};
use lofty::{config::ParseOptions, probe::Probe};
use std::{
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use types::*;
pub(crate) fn is_virtual_cue_track_uri(uri: &str) -> bool {
    let path = Path::new(uri);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let Some(track) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(number) = track.strip_prefix("track") else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some(cue_directory) = path.parent() else {
        return false;
    };
    let is_cue = cue_directory
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".cue"));
    let has_physical_folder = cue_directory
        .parent()
        .is_some_and(|folder| !folder.as_os_str().is_empty());
    is_cue && has_physical_folder
}

// One physical metadata inspection. Caller timeouts do not release this permit
// while a filesystem/Lofty operation and its held file handles still exist.
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self, LocalTrackTagsError> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| LocalTrackTagsError::task_failed())?;
        Ok(Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
fn bounded<T: Send + 'static, E: Send + 'static>(
    timeout: Duration,
    error: fn() -> E,
    work: impl FnOnce(crate::windows_io::Job) -> Result<T, E> + Send + 'static,
) -> Result<T, E> {
    let permit = Permit::acquire().map_err(|_| error())?;
    crate::windows_io::read(timeout, move |job| {
        let _permit = permit;
        work(job)
    })
    .map_err(|_| error())?
}
fn info_error(error: InfoError) -> LocalTrackTagsError {
    let code = match error {
        InfoError::InvalidLocalPath => LocalTrackTagsErrorCode::InvalidLocalPath,
        InfoError::OutsideConfiguredRoots => LocalTrackTagsErrorCode::OutsideConfiguredRoots,
        InfoError::FileNotFound => LocalTrackTagsErrorCode::FileNotFound,
        InfoError::PermissionDenied => LocalTrackTagsErrorCode::PermissionDenied,
        InfoError::Stale => LocalTrackTagsErrorCode::RevisionConflict,
        _ => LocalTrackTagsErrorCode::ReadFailed,
    };
    LocalTrackTagsError::new(
        code,
        "Não foi possível concluir a inspeção das tags locais.",
    )
}
struct CancelReader<R, F> {
    inner: R,
    current: F,
}
impl<R: Read, F: Fn() -> bool> Read for CancelReader<R, F> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if !(self.current)() {
            return Err(io::Error::other("metadata_retired"));
        }
        let n = self.inner.read(bytes)?;
        if !(self.current)() {
            return Err(io::Error::other("metadata_retired"));
        }
        Ok(n)
    }
}
impl<R: Seek, F: Fn() -> bool> Seek for CancelReader<R, F> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if !(self.current)() {
            return Err(io::Error::other("metadata_retired"));
        }
        let n = self.inner.seek(pos)?;
        if !(self.current)() {
            return Err(io::Error::other("metadata_retired"));
        }
        Ok(n)
    }
}
fn read_tags(
    reader: impl Read + Seek,
    revision: String,
) -> Result<LocalTrackTags, LocalTrackTagsError> {
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false);
    let probe = Probe::new(BufReader::new(reader))
        .options(options)
        .guess_file_type()
        .map_err(|_| {
            LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::ReadFailed,
                "Não foi possível identificar o arquivo local.",
            )
        })?;
    if probe.file_type().is_none() {
        return Err(LocalTrackTagsError::unsupported(
            LocalTrackTagsErrorCode::UnsupportedFormat,
            "unknown",
            "O formato do arquivo não é suportado para inspeção de tags.",
        ));
    }
    let tagged = probe.read().map_err(|_| {
        LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "Não foi possível ler as tags do arquivo local.",
        )
    })?;
    let tags = types::local_track_tags_from_file(&tagged, revision);
    // Reject excessive values; do not truncate them into false tag evidence.
    for text in [
        &tags.title,
        &tags.artist,
        &tags.album,
        &tags.album_artist,
        &tags.genre,
    ] {
        if text.as_ref().is_some_and(|v| v.len() > 4096) {
            return Err(LocalTrackTagsError::task_failed());
        }
    }
    if tags.date.as_ref().is_some_and(|v| v.len() > 128) {
        return Err(LocalTrackTagsError::task_failed());
    }
    Ok(tags)
}
fn read_one(
    library: &Library,
    uri: &str,
    job: &crate::windows_io::Job,
    token: &str,
) -> Result<LocalTrackTags, LocalTrackTagsError> {
    if is_virtual_cue_track_uri(uri) {
        return Err(LocalTrackTagsError::unsupported(
            LocalTrackTagsErrorCode::CueVirtual,
            "cue_virtual",
            "Faixas virtuais CUE não possuem um arquivo individual editável.",
        ));
    }
    let current = || job.current() && library.check_metadata_snapshot(token).is_ok();
    crate::windows_file_info::inspect_catalog(library, uri, &current, |file, _, revision| {
        Ok(read_tags(
            CancelReader {
                inner: crate::windows_file_info::Budget::new(file),
                current: &current,
            },
            revision,
        ))
    })
    .map_err(info_error)?
}
pub(crate) fn inspect_track(
    library: Library,
    uri: String,
) -> Result<LocalTrackTags, LocalTrackTagsError> {
    bounded(
        crate::windows_io::READ_TIMEOUT,
        LocalTrackTagsError::task_failed,
        move |job| {
            let token = library
                .metadata_snapshot_token()
                .map_err(|_| LocalTrackTagsError::state_unavailable())?;
            let result = read_one(&library, &uri, &job, &token);
            if !job.current() || library.check_metadata_snapshot(&token).is_err() {
                return Err(LocalTrackTagsError::task_failed());
            }
            result
        },
    )
}
fn groups(
    library: &Library,
    album: &str,
    token: &str,
) -> Result<Option<Vec<catalog::LocalAlbumDiscTracks>>, String> {
    if album.len() != 64 || !album.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let Some(mut tracks) = library.metadata_members(album, token)? else {
        return Ok(None);
    };
    tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
    let mut groups = std::collections::BTreeMap::<u32, Vec<catalog::LocalItem>>::new();
    for track in tracks {
        groups
            .entry(track.disc)
            .or_default()
            .push(catalog::LocalItem {
                item_type: "file".into(),
                path: crate::windows_mpd::library::track_uri(&track),
                name: track.file_name,
                title: Some(track.title),
                artist: Some(track.artist),
                album: Some(track.album),
                duration: None,
            });
    }
    Ok(Some(
        groups
            .into_iter()
            .map(|(number, tracks)| catalog::LocalAlbumDiscTracks {
                disc: catalog::LocalAlbumDisc {
                    number,
                    label: format!("CD {number}"),
                    folder_path: format!("windows/{album}/{number}"),
                    track_count: tracks.len(),
                },
                tracks,
            })
            .collect(),
    ))
}

fn inspection(album: String, tracks: Vec<LocalAlbumTrackInspection>) -> LocalAlbumTagInspection {
    let physical_track_count = tracks
        .iter()
        .filter(|t| t.kind == LocalAlbumTrackKind::Physical)
        .count();
    let cue_virtual_count = tracks.len() - physical_track_count;
    let inspection_complete = tracks
        .iter()
        .all(|t| t.kind == LocalAlbumTrackKind::CueVirtual || t.tags.is_some());
    let fields = LocalAlbumTagFields {
        album: types::summarize_album_field(&tracks, |t| t.album.as_deref()),
        album_artist: types::summarize_album_field(&tracks, |t| t.album_artist.as_deref()),
        date: types::summarize_album_field(&tracks, |t| t.date.as_deref()),
        genre: types::summarize_album_field(&tracks, |t| t.genre.as_deref()),
    };
    LocalAlbumTagInspection {
        album_id: album,
        tracks,
        fields,
        physical_track_count,
        cue_virtual_count,
        inspection_complete,
    }
}
pub(crate) fn inspect_album(
    library: Library,
    album: String,
) -> Result<LocalAlbumTagInspection, LocalTrackTagsError> {
    bounded(
        Duration::from_secs(10),
        LocalTrackTagsError::task_failed,
        move |job| {
            let token = library
                .metadata_snapshot_token()
                .map_err(|_| LocalTrackTagsError::state_unavailable())?;
            let groups = groups(&library, &album, &token)
                .map_err(|_| LocalTrackTagsError::album_catalog_failed())?
                .ok_or_else(LocalTrackTagsError::album_not_found)?;
            let mut tracks = Vec::new();
            for track in groups.into_iter().flat_map(|g| g.tracks) {
                if !job.current() || library.check_metadata_snapshot(&token).is_err() {
                    return Err(LocalTrackTagsError::task_failed());
                }
                let uri = track.path;
                let kind = if is_virtual_cue_track_uri(&uri) {
                    LocalAlbumTrackKind::CueVirtual
                } else {
                    LocalAlbumTrackKind::Physical
                };
                let (tags, issue) = match read_one(&library, &uri, &job, &token) {
                    Ok(tags) => (Some(tags), None),
                    Err(error) => (None, Some(error)),
                };
                tracks.push(LocalAlbumTrackInspection {
                    uri,
                    kind,
                    tags,
                    issue,
                });
            }
            if !job.current() || library.check_metadata_snapshot(&token).is_err() {
                return Err(LocalTrackTagsError::task_failed());
            }
            Ok(inspection(album, tracks))
        },
    )
}
pub(crate) fn diagnose_album(
    library: Library,
    album: String,
) -> Result<doctor::Report, doctor::DoctorError> {
    bounded(
        Duration::from_secs(10),
        || doctor::DoctorError::TaskFailed,
        move |job| {
            let token = library
                .metadata_snapshot_token()
                .map_err(|_| doctor::DoctorError::StateUnavailable)?;
            let result = doctor::inspect_album_with(
                &album,
                || groups(&library, &album, &token),
                |uri| read_one(&library, uri, &job, &token),
            );
            if !job.current() || library.check_metadata_snapshot(&token).is_err() {
                return Err(doctor::DoctorError::TaskFailed);
            }
            result
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, time::Instant};
    struct Fixture {
        root: std::path::PathBuf,
        library: Library,
    }
    impl Fixture {
        fn new() -> Self {
            let mut n = [0; 12];
            getrandom::fill(&mut n).unwrap();
            let root = std::env::temp_dir().join(format!("sonante-metadata-own-{n:x?}"));
            fs::create_dir(&root).unwrap();
            let media = root.join("Álbum próprio");
            fs::create_dir(&media).unwrap();
            let library = Library::new(root.join("settings.json"));
            Self { root, library }
        }
        fn media(&self) -> std::path::PathBuf {
            self.root.join("Álbum próprio")
        }
        fn scan(&self) {
            self.library.select(self.media()).unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                let status = self.library.status().unwrap();
                if status.phase == "ready" {
                    break;
                }
                assert!(Instant::now() < until, "own fixture scan");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        fn album(&self) -> String {
            self.library.albums("", 0, 10).unwrap().items[0].id.clone()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.library.stop();
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    // Original synthetic FLAC fixture format, no real music or decoding/playback.
    fn flac(path: &Path, comments: &[&str]) {
        let mut bytes = b"fLaC".to_vec();
        let mut stream = Vec::new();
        stream.extend(4096_u16.to_be_bytes());
        stream.extend(4096_u16.to_be_bytes());
        stream.extend([0; 6]);
        let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 44_100_u64;
        stream.extend(packed.to_be_bytes());
        stream.extend([0; 16]);
        bytes.push(if comments.is_empty() { 0x80 } else { 0 });
        bytes.extend([0, 0, stream.len() as u8]);
        bytes.extend(stream);
        if !comments.is_empty() {
            let vendor = b"Sonante test";
            let mut block = Vec::new();
            block.extend((vendor.len() as u32).to_le_bytes());
            block.extend(vendor);
            block.extend((comments.len() as u32).to_le_bytes());
            for c in comments {
                block.extend((c.len() as u32).to_le_bytes());
                block.extend(c.as_bytes());
            }
            bytes.push(0x84);
            bytes.extend(&(block.len() as u32).to_be_bytes()[1..]);
            bytes.extend(block);
        }
        bytes.extend(b"\xff\xf8SONANTE_TEST_AUDIO_PAYLOAD");
        let mut file = fs::File::create(path).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
    }
    #[test]
    fn read_original_flac_values_and_diagnose_current_catalog_without_any_writes() {
        let f = Fixture::new();
        let path = f.media().join("01.flac");
        flac(
            &path,
            &[
                "TITLE=Canção",
                "ARTIST=Artista",
                "ALBUM=Álbum",
                "ALBUMARTIST=",
                "DATE=2026-02-30",
                "TRACKNUMBER=7",
                "TRACKTOTAL=2",
                "DISCNUMBER=1",
                "DISCTOTAL=1",
            ],
        );
        let before = fs::read(&path).unwrap();
        f.scan();
        let album = f.album();
        let track = f.library.tracks(&album).unwrap().remove(0);
        let uri = crate::windows_mpd::library::track_uri(&track);
        let tags = inspect_track(f.library.clone(), uri.clone()).unwrap();
        assert_eq!(tags.title.as_deref(), Some("Canção"));
        assert_eq!(tags.album_artist.as_deref(), Some(""));
        assert_eq!(tags.date.as_deref(), Some("2026-02-30"));
        assert!(tags.readable);
        assert!(!tags.writable);
        assert!(!tags.editable);
        assert!(tags.revision.starts_with("windows-tags-v1."));
        let inspection = inspect_album(f.library.clone(), album.clone()).unwrap();
        assert_eq!(inspection.physical_track_count, 1);
        assert!(inspection.inspection_complete);
        assert_eq!(inspection.tracks[0].uri, uri);
        assert_eq!(
            inspection.fields.album_artist.state,
            LocalAlbumFieldState::Common
        );
        assert_eq!(inspection.fields.album_artist.value.as_deref(), Some(""));
        let report = diagnose_album(f.library.clone(), album).unwrap();
        assert_eq!(report.coverage.physical_tracks_read, 1);
        for code in [
            doctor::FindingCode::AlbumArtistEmpty,
            doctor::FindingCode::DateUninterpretable,
            doctor::FindingCode::TrackExceedsTotal,
        ] {
            assert!(report.findings.iter().any(|v| v.code == code));
        }
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(f.media()).unwrap().count(), 1);
    }
    #[test]
    fn failed_tag_reads_are_partial_and_never_evidence_of_absent_tags() {
        let f = Fixture::new();
        let path = f.media().join("01.flac");
        fs::write(&path, b"not a media format").unwrap();
        f.scan();
        let album = f.album();
        let report = diagnose_album(f.library.clone(), album).unwrap();
        assert_eq!(report.coverage.physical_tracks_read, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(report.coverage.partial);
        assert!(report.findings.is_empty());
        assert_eq!(fs::read(&path).unwrap(), b"not a media format");
    }
    #[test]
    fn disabled_local_source_stale_membership_and_raw_paths_cannot_read_tags() {
        let f = Fixture::new();
        let path = f.media().join("01.flac");
        flac(&path, &["TITLE=Own", "ARTIST=Artist", "ALBUM=Album"]);
        f.scan();
        let album = f.album();
        let track = f.library.tracks(&album).unwrap().remove(0);
        let uri = crate::windows_mpd::library::track_uri(&track);
        let before = fs::read(&path).unwrap();
        assert!(inspect_track(f.library.clone(), path.to_string_lossy().into()).is_err());
        assert!(matches!(
            diagnose_album(f.library.clone(), "b".repeat(64)),
            Err(doctor::DoctorError::AlbumNotFound)
        ));
        f.library
            .configure(
                None,
                crate::windows_mpd::library::Sources {
                    local: false,
                    navidrome: true,
                    plex: false,
                },
            )
            .unwrap();
        assert!(inspect_track(f.library.clone(), uri).is_err());
        assert!(diagnose_album(f.library.clone(), album).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
    #[test]
    fn catalog_change_during_a_held_read_discards_the_result_before_publication() {
        let f = Fixture::new();
        let path = f.media().join("01.flac");
        flac(&path, &["ALBUM=Own", "ARTIST=Artist"]);
        f.scan();
        let album = f.album();
        let track = f.library.tracks(&album).unwrap().remove(0);
        let uri = crate::windows_mpd::library::track_uri(&track);
        let token = f.library.metadata_snapshot_token().unwrap();
        let before = fs::read(&path).unwrap();
        let result = crate::windows_file_info::inspect_catalog(
            &f.library,
            &uri,
            || f.library.check_metadata_snapshot(&token).is_ok(),
            |_, _, _| {
                f.library
                    .configure(
                        None,
                        crate::windows_mpd::library::Sources {
                            local: false,
                            navidrome: true,
                            plex: false,
                        },
                    )
                    .unwrap();
                Ok("late metadata must be discarded")
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
    #[test]
    fn excessive_tag_text_is_rejected_not_truncated_into_absence_or_matching_metadata() {
        let f = Fixture::new();
        let path = f.media().join("01.flac");
        let title = format!("TITLE={}", "x".repeat(4097));
        flac(&path, &[&title]);
        let mut file = fs::File::open(path).unwrap();
        assert!(read_tags(&mut file, "own-revision".into()).is_err());
    }
    #[test]
    fn timeout_keeps_metadata_admission_until_the_actual_worker_returns() {
        let (release, wait) = std::sync::mpsc::channel();
        let (started, ready) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            bounded(
                Duration::from_millis(40),
                || "timeout",
                move |job| {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    assert!(!job.current());
                    Ok(9)
                },
            )
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(caller.join().unwrap(), Err("timeout"));
        assert_eq!(
            bounded(Duration::from_secs(1), || "busy", |_| Ok(1)),
            Err("busy")
        );
        release.send(()).unwrap();
        let end = Instant::now() + Duration::from_secs(1);
        while ACTIVE.load(Ordering::Acquire) {
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        assert_eq!(
            bounded(Duration::from_secs(1), || "failed", |_| Ok(1)),
            Ok(1)
        );
    }
    #[test]
    fn cancellation_checks_after_a_read_never_publish_late_payload() {
        struct Late(std::sync::Arc<AtomicBool>);
        impl Read for Late {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.0.store(false, Ordering::Release);
                out[0] = 7;
                Ok(1)
            }
        }
        let current = std::sync::Arc::new(AtomicBool::new(true));
        let c = current.clone();
        let mut reader = CancelReader {
            inner: Late(current),
            current: move || c.load(Ordering::Acquire),
        };
        assert!(reader.read(&mut [0; 1]).is_err());
    }
}
