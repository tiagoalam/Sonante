//! Bounded, replaceable discoveries. This is never a confirmed LocalAlbum
//! catalog: there are no playback URIs, stable album IDs or persistence.
use crate::{audio::AudioEngine, config_save_operation::PreviewLease, local_media};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

const TRACK_LIMIT: usize = 128;
const BYTE_LIMIT: usize = 128 * 1024;
const LINE_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreviewError {
    Unavailable,
    Busy,
    ReadFailed,
    InvalidResponse,
    LimitExceeded,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    pub initial: bool,
    pub socket: String,
    pub sources: Vec<String>,
    pub job: Option<u64>,
    filters: Vec<String>,
    pub artwork: Option<crate::local_import_artwork::ArtworkScope>,
}

impl Scope {
    pub(crate) fn initial(socket: &str, sources: &[String]) -> Result<Self, PreviewError> {
        if sources.is_empty() || sources.len() > 65_536 { return Err(PreviewError::LimitExceeded); }
        let mut filters = Vec::new();
        let mut unique = BTreeSet::new();
        for chunk in sources.chunks(8) {
            let scope = Self::new(socket, chunk)?;
            for source in chunk { if !unique.insert(source) { return Err(PreviewError::InvalidResponse); } }
            filters.extend(scope.filters);
        }
        Ok(Self { initial: true, socket: socket.into(), sources: sources.to_vec(), job: None, filters, artwork: None })
    }
    pub(crate) fn new(socket: &str, sources: &[String]) -> Result<Self, PreviewError> {
        if sources.is_empty() || sources.len() > 8 {
            return Err(PreviewError::LimitExceeded);
        }
        let mut filters = Vec::new();
        let mut unique = BTreeSet::new();
        for id in sources {
            if id.len() > 256 {
                return Err(PreviewError::LimitExceeded);
            }
            if !local_media::is_safe_relative_mpd_uri(id) || id.contains('/') || !unique.insert(id)
            {
                return Err(PreviewError::InvalidResponse);
            }
            filters.push(
                AudioEngine::local_album_base_filter(id)
                    .map_err(|_| PreviewError::InvalidResponse)?,
            );
        }
        if filters.iter().map(String::len).sum::<usize>() > LINE_LIMIT {
            return Err(PreviewError::LimitExceeded);
        }
        Ok(Self {
            initial: false,
            socket: socket.into(),
            sources: sources.to_vec(),
            job: None,
            filters,
            artwork: None,
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AlbumDiscovery {
    /// Opaque, operation-scoped grouping identity; never a confirmed album ID.
    pub provisional_group_key: String,
    pub image: Option<String>,
    pub artwork_ref: Option<String>,
    pub source_id: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub discs: Vec<String>,
    pub observed_tracks: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct Discovery {
    pub operation_id: String,
    pub initial: bool,
    pub provisional: bool,
    pub playable: bool,
    /// Always false: a short mutable page cannot prove catalog completeness.
    pub complete: bool,
    pub sampled_tracks: usize,
    pub sample_limit: usize,
    pub albums: Vec<AlbumDiscovery>,
}

pub(crate) fn read(id: &str, lease: &PreviewLease) -> Result<Discovery, PreviewError> {
    read_with(id, lease, |command, deadline| {
        exchange(&lease.scope.socket, command, deadline)
    })
}

pub(crate) fn read_with(
    id: &str,
    lease: &PreviewLease,
    mut query: impl FnMut(&str, Instant) -> Result<Vec<String>, PreviewError>,
) -> Result<Discovery, PreviewError> {
    lease.ensure_active()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let job = lease.scope.job.ok_or(PreviewError::Unavailable)?;
    let offsets = lease.offsets()?;
    let mut next_offsets = offsets.clone();
    let status = query("status", deadline)?;
    // MPD may be working on its automatic first scan while our rescan is
    // queued. Only this initial, operation-owned generation admits that job;
    // both ends of a sample must still observe the exact SAME active job.
    let job = if lease.scope.initial {
        active_job(&status).ok_or(PreviewError::Unavailable)?
    } else { job };
    check_job(&status, job)?;
    // At most eight small windows restricted to individual new sources. No
    // count or stable continuation cursor: every request replaces the sample.
    let source_start = lease.source_start()?;
    let source_count = lease.scope.sources.len().min(8);
    let per_source = TRACK_LIMIT / source_count;
    let mut albums = Vec::new();
    let mut sampled_tracks = 0;
    let mut artwork_budget = 2;
    for n in 0..source_count {
        let index = (source_start + n) % lease.scope.sources.len();
        let source = &lease.scope.sources[index];
        let filter = &lease.scope.filters[index];
        let start = offsets[index];
        lease.ensure_active()?;
        let end = start
            .checked_add(per_source)
            .ok_or(PreviewError::LimitExceeded)?;
        let lines = query(&format!("find {filter} window {start}:{end}"), deadline)?;
        let (rows, count) =
            discoveries_with_artwork(id, lines, std::slice::from_ref(source), &mut |folder| {
                lease.ensure_active().ok()?;
                if lease.scope.initial { return None; }
                let artwork = lease.scope.artwork.as_ref()?;
                if artwork_budget == 0 || Instant::now() >= deadline {
                    return None;
                }
                artwork_budget -= 1;
                artwork.read(folder)
            })?;
        if count > per_source {
            return Err(PreviewError::LimitExceeded);
        }
        sampled_tracks += count;
        albums.extend(rows);
        // Revisit the beginning after reaching the currently observable end.
        // Shrinkage/CUE replacement can invalidate offsets at any time.
        next_offsets[index] = if count < per_source { 0 } else { end };
    }
    check_job(&query("status", deadline)?, job)?;
    lease.commit_offsets(&offsets, next_offsets)?;
    Ok(Discovery {
        operation_id: id.into(),
        initial: lease.scope.initial,
        provisional: true,
        playable: false,
        complete: false,
        sampled_tracks,
        sample_limit: TRACK_LIMIT,
        albums,
    })
}

fn check_job(lines: &[String], job: u64) -> Result<(), PreviewError> {
    if active_job(lines) == Some(job) { Ok(()) } else { Err(PreviewError::Unavailable) }
}

fn active_job(lines: &[String]) -> Option<u64> {
    let mut jobs = lines
        .iter()
        .filter_map(|line| line.strip_prefix("updating_db: "));
    let current = jobs.next().and_then(|value| value.parse::<u64>().ok());
    if current == Some(0) || jobs.next().is_some() { None } else { current }
}

// Dedicated stricter read budget; ordinary audio commands keep their existing
// transport semantics. ACK, EOF, timeout and partial replies all fail closed;
// no raw MPD errors (potential paths/credentials) escape this diagnostic API.
fn exchange(socket: &str, command: &str, deadline: Instant) -> Result<Vec<String>, PreviewError> {
    let stream = UnixStream::connect(socket).map_err(|_| PreviewError::ReadFailed)?;
    let mut reader = BufReader::new(stream);
    let mut bytes = 0;
    let greeting = line(&mut reader, deadline, &mut bytes)?;
    if !greeting.starts_with("OK MPD ") {
        return Err(PreviewError::InvalidResponse);
    }
    reader
        .get_ref()
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|_| PreviewError::ReadFailed)?;
    writeln!(reader.get_mut(), "{command}").map_err(|_| PreviewError::ReadFailed)?;
    let mut lines = Vec::new();
    loop {
        let value = line(&mut reader, deadline, &mut bytes)?;
        if value == "OK" {
            return Ok(lines);
        }
        if value.starts_with("ACK") {
            return Err(PreviewError::ReadFailed);
        }
        lines.push(value);
    }
}

fn line(
    reader: &mut BufReader<UnixStream>,
    deadline: Instant,
    bytes: &mut usize,
) -> Result<String, PreviewError> {
    let mut value = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(PreviewError::ReadFailed)?;
        reader
            .get_ref()
            .set_read_timeout(Some(remaining.min(Duration::from_millis(500))))
            .map_err(|_| PreviewError::ReadFailed)?;
        let chunk = reader.fill_buf().map_err(|_| PreviewError::ReadFailed)?;
        if chunk.is_empty() {
            return Err(PreviewError::ReadFailed);
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let take = newline.map_or(chunk.len(), |pos| pos + 1);
        *bytes += take;
        if *bytes > BYTE_LIMIT || value.len() + take > LINE_LIMIT {
            return Err(PreviewError::LimitExceeded);
        }
        value.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if newline.is_some() {
            let text = String::from_utf8(value).map_err(|_| PreviewError::InvalidResponse)?;
            return Ok(text.trim_end_matches(['\r', '\n']).into());
        }
    }
}

fn display(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 256
        || value.contains("://")
        || value.to_ascii_lowercase().contains("x-plex-token")
        || value.chars().any(char::is_control)
    {
        None
    } else {
        Some(value.into())
    }
}

// A presentation-only logical reference, not a physical path or playback ID.
// No per-album native registry: the existing operation/source anchors authorize
// only the cover reader's finite, verified image-name allowlist.
fn artwork_reference(operation: &str, folder: &Path) -> Option<String> {
    let folder = folder.to_str()?;
    if folder.len() > 4096 || !local_media::is_safe_relative_mpd_uri(folder) { return None; }
    let encoded = folder.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
    Some(format!("cover-v1:{operation}:{encoded}"))
}

pub(crate) fn read_artwork(id: &str, reference: &str, lease: &PreviewLease) -> Result<Option<String>, PreviewError> {
    lease.ensure_active()?;
    if lease.scope.job.is_none() { return Err(PreviewError::Unavailable); }
    let prefix = format!("cover-v1:{id}:");
    let encoded = reference.strip_prefix(&prefix).ok_or(PreviewError::InvalidResponse)?;
    if encoded.is_empty() || encoded.len() > 8192 || encoded.len() % 2 != 0
        || !encoded.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(PreviewError::InvalidResponse);
    }
    let bytes = encoded.as_bytes().chunks_exact(2).map(|pair| {
        u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
    }).collect::<Vec<_>>();
    let folder = std::str::from_utf8(&bytes).map_err(|_| PreviewError::InvalidResponse)?;
    let source = folder.split('/').next().ok_or(PreviewError::InvalidResponse)?;
    if !local_media::is_safe_relative_mpd_uri(folder) || !lease.scope.sources.iter().any(|s| s == source) {
        return Err(PreviewError::InvalidResponse);
    }
    let image = lease.scope.artwork.as_ref().and_then(|artwork| artwork.read(Path::new(folder)));
    lease.ensure_active()?;
    Ok(image)
}

fn provisional_group_key(
    operation: &str,
    source: &str,
    folder: &Path,
    album: Option<&str>,
    artist: Option<&str>,
) -> String {
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    let mut hash = Sha256::new();
    hash.update(b"sonante-local-import-group-v1\0");
    field(&mut hash, operation.as_bytes());
    field(&mut hash, source.as_bytes());
    // Components match Path's grouping equality, without leaking the logical
    // directory. CUE normalization is performed by the existing caller.
    hash.update((folder.components().count() as u64).to_be_bytes());
    for component in folder.components() {
        field(&mut hash, component.as_os_str().as_encoded_bytes());
    }
    for value in [album, artist] {
        hash.update([u8::from(value.is_some())]);
        if let Some(value) = value {
            field(&mut hash, value.as_bytes());
        }
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
fn discoveries(
    operation: &str,
    lines: Vec<String>,
    sources: &[String],
) -> Result<(Vec<AlbumDiscovery>, usize), PreviewError> {
    discoveries_with_artwork(operation, lines, sources, &mut |_| None)
}

fn discoveries_with_artwork(
    operation: &str,
    lines: Vec<String>,
    sources: &[String],
    artwork: &mut impl FnMut(&Path) -> Option<String>,
) -> Result<(Vec<AlbumDiscovery>, usize), PreviewError> {
    let mut tracks: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    let mut unique = BTreeSet::new();
    for line in lines {
        let (key, value) = line.split_once(": ").ok_or(PreviewError::InvalidResponse)?;
        if key == "file" {
            if tracks.len() == TRACK_LIMIT {
                return Err(PreviewError::LimitExceeded);
            }
            let id = value
                .split('/')
                .next()
                .ok_or(PreviewError::InvalidResponse)?;
            if !sources.iter().any(|source| source == id)
                || !local_media::is_safe_relative_mpd_uri(value)
                || !local_media::is_catalog_track_uri(value)
                || !unique.insert(value.to_string())
            {
                return Err(PreviewError::InvalidResponse);
            }
            tracks.push((value.into(), BTreeMap::new()));
        } else {
            let (_, tags) = tracks.last_mut().ok_or(PreviewError::InvalidResponse)?;
            if matches!(key, "Album" | "AlbumArtist" | "Artist" | "Disc") {
                if let Some(text) = display(value) {
                    tags.entry(key.into()).or_insert(text);
                }
            }
        }
    }
    let sampled = tracks.len();
    let mut albums = BTreeMap::new();
    for (uri, tags) in tracks {
        let mut folder = Path::new(&uri)
            .parent()
            .ok_or(PreviewError::InvalidResponse)?;
        if local_media::is_virtual_cue_track_uri(&uri) {
            folder = folder.parent().ok_or(PreviewError::InvalidResponse)?;
        }
        let source = uri.split('/').next().unwrap_or_default().to_string();
        let album = tags.get("Album").cloned();
        let artist = tags
            .get("AlbumArtist")
            .or_else(|| tags.get("Artist"))
            .cloned();
        // The key follows this exact provisional grouping across pages; it
        // does not promise definitive multidisc/CUE catalog identity.
        let row = albums
            .entry((folder.to_path_buf(), album.clone(), artist.clone()))
            .or_insert_with(|| AlbumDiscovery {
                provisional_group_key: provisional_group_key(
                    operation,
                    &source,
                    folder,
                    album.as_deref(),
                    artist.as_deref(),
                ),
                image: artwork(folder),
                artwork_ref: artwork_reference(operation, folder),
                source_id: source,
                album,
                artist,
                discs: Vec::new(),
                observed_tracks: 0,
            });
        row.observed_tracks += 1;
        if let Some(disc) = tags.get("Disc") {
            if !row.discs.contains(disc) {
                row.discs.push(disc.clone());
            }
        }
    }
    Ok((albums.into_values().collect(), sampled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_save_operation::{Coordinator, Phase};
    use std::sync::Arc;

    const FIND: &str = "find \"(base 'Nova')\" window 0:128";
    const ACTIVE: &str = "updating_db: 7\nstate: play\nOK\n";
    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn page(start: usize, count: usize, album: &str) -> String {
        let mut response = (start..start + count)
            .map(|n| format!("file: Nova/{n:04}.flac\nAlbum: {album}\n"))
            .collect::<String>();
        response.push_str("OK\n");
        response
    }

    #[test]
    fn preview_artwork_is_bounded_per_sample_and_failure_does_not_change_save_or_pagination() {
        let fixture = crate::local_import_artwork::tests::Fixture::new();
        for folder in ["A", "B", "C"] {
            fixture.sidecar(
                folder,
                "cover.png",
                &crate::local_import_artwork::tests::png(),
            );
        }
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        op.enable_preview_artwork(crate::local_import_artwork::ArtworkScope::new(
            &fixture.mirror,
            &fixture.manifest,
            &["Nova".into()],
        ));
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        assert!(lease.scope.artwork.is_some());
        let sample = read_with(&accepted.id, &lease, |command, _| {
            if command == "status" { return Ok(lines("updating_db: 7")); }
            assert_eq!(command, FIND);
            Ok(lines("file: Nova/A/a.flac\nAlbum: A\nfile: Nova/B/image.CUE/track0001\nAlbum: B\nfile: Nova/C/c.flac\nAlbum: C"))
        }).unwrap();
        assert_eq!(
            sample
                .albums
                .iter()
                .filter(|row| row.image.is_some())
                .count(),
            2
        );
        assert!(sample.albums[1].image.is_some()); // CUE uses its real album folder.
        assert_eq!(lease.offsets().unwrap(), [0]);
        std::fs::write(fixture.physical.join("A/cover.png"), b"not an image").unwrap();
        let sample = read_with(&accepted.id, &lease, |command, _| {
            if command == "status" {
                Ok(lines("updating_db: 7"))
            } else {
                Ok(lines("file: Nova/A/a.flac\nAlbum: A"))
            }
        })
        .unwrap();
        assert!(sample.albums[0].image.is_none());
        assert!(sample.provisional && !sample.playable && !sample.complete);
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Indexing);
        assert_eq!(lease.offsets().unwrap(), [0]);
    }

    #[test]
    fn preview_artwork_job_or_operation_invalidation_never_returns_decoded_images() {
        for boundary in 0..5 {
            let fixture = crate::local_import_artwork::tests::Fixture::new();
            fixture.sidecar("A", "cover.png", &crate::local_import_artwork::tests::png());
            let state = Arc::new(Coordinator::new());
            let (accepted, op) = state.reserve().unwrap();
            op.enable_addition_preview("unused", &["Nova".into()])
                .unwrap();
            op.enable_preview_artwork(crate::local_import_artwork::ArtworkScope::new(
                &fixture.mirror,
                &fixture.manifest,
                &["Nova".into()],
            ));
            op.phase(Phase::Indexing);
            let mut job = Some(op.preview_job(7));
            let lease = state.reserve_preview(&accepted.id).unwrap();
            let mut op = Some(op);
            let mut statuses = 0;
            let mut next_job = None;
            let mut next_operation = None;
            let result = read_with(&accepted.id, &lease, |command, _| {
                if command == "status" {
                    statuses += 1;
                    if statuses == 2 {
                        match boundary {
                            0 => drop(job.take()),
                            1 => next_job = Some(op.as_ref().unwrap().preview_job(8)),
                            2 => op.take().unwrap().finish(Err("uncertain".into()), true),
                            3 => {
                                op.take().unwrap().finish(Ok(()), false);
                                next_operation = Some(state.reserve().unwrap().1);
                            }
                            _ => state.request_shutdown(|| {}),
                        }
                    }
                    Ok(lines("updating_db: 7"))
                } else {
                    Ok(lines("file: Nova/A/a.flac\nAlbum: A"))
                }
            });
            assert!(matches!(result, Err(PreviewError::Unavailable)));
            assert!(lease.offsets().is_err());
        }
    }

    #[test]
    fn preview_group_key_survives_successive_rotating_pages() {
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let mut key = None;
        for (start, count, next) in [(0, 128, 128), (128, 128, 256), (256, 44, 0)] {
            let sample = read_with(&accepted.id, &lease, |command, _| {
                if command == "status" {
                    Ok(lines("updating_db: 7"))
                } else {
                    assert_eq!(
                        command,
                        format!("find \"(base 'Nova')\" window {start}:{}", start + 128)
                    );
                    Ok(lines(&page(start, count, "Mesmo álbum"))
                        .into_iter()
                        .filter(|line| line != "OK")
                        .collect())
                }
            })
            .unwrap();
            assert_eq!(sample.sampled_tracks, count);
            assert_eq!(sample.sample_limit, 128);
            assert_eq!(sample.albums.len(), 1);
            let observed = &sample.albums[0].provisional_group_key;
            assert_eq!(key.get_or_insert_with(|| observed.clone()), observed);
            assert_eq!(lease.offsets().unwrap(), [next]);
        }
    }

    #[test]
    fn preview_group_keys_distinguish_homonymous_directories_and_operations() {
        let response = lines("file: Nova/Primeiro/a.flac\nAlbum: Igual\nArtist: Igual\nfile: Nova/Segundo/b.flac\nAlbum: Igual\nArtist: Igual");
        let (first, _) = discoveries("1", response.clone(), &["Nova".into()]).unwrap();
        let (second, _) = discoveries("2", response, &["Nova".into()]).unwrap();
        assert_eq!(first.len(), 2);
        assert_ne!(
            first[0].provisional_group_key,
            first[1].provisional_group_key
        );
        for (a, b) in first.iter().zip(&second) {
            assert_ne!(a.provisional_group_key, b.provisional_group_key);
        }
    }

    #[test]
    fn preview_group_keys_preserve_cue_normalization_and_separate_disc_directories() {
        let physical =
            lines("file: Nova/Album/CD1/image.flac\nAlbum: Mesmo\nAlbumArtist: Banda\nDisc: 1");
        let virtual_track = lines(
            "file: Nova/Album/CD1/image.CUE/track0001\nAlbum: Mesmo\nAlbumArtist: Banda\nDisc: 1",
        );
        let other_disc =
            lines("file: Nova/Album/CD2/song.flac\nAlbum: Mesmo\nAlbumArtist: Banda\nDisc: 2");
        let sample = |response| discoveries("1", response, &["Nova".into()]).unwrap().0;
        let a = sample(physical);
        let b = sample(virtual_track);
        let c = sample(other_disc);
        assert_eq!(a[0].provisional_group_key, b[0].provisional_group_key);
        assert_ne!(a[0].provisional_group_key, c[0].provisional_group_key);
        let json = serde_json::to_string(&b).unwrap();
        for secret in ["Nova/", "CD1", "image.CUE", "track0001", "file:"] {
            assert!(!json.contains(secret));
        }
        assert_eq!(b[0].provisional_group_key.len(), 64);
        assert!(b[0]
            .provisional_group_key
            .bytes()
            .all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn preview_group_key_encoding_is_unambiguous() {
        let key = |operation, source, folder, album, artist| {
            provisional_group_key(operation, source, Path::new(folder), album, artist)
        };
        assert_ne!(
            key("1", "ab", "c", None, None),
            key("1", "a", "bc", None, None)
        );
        assert_ne!(
            key("1", "S", "S/A", Some("ab"), Some("c")),
            key("1", "S", "S/A", Some("a"), Some("bc"))
        );
        assert_ne!(
            key("1", "S", "S/A", None, None),
            key("1", "S", "S/A", Some(""), None)
        );
        assert_ne!(
            key("1", "S", "S/A", Some("X"), None),
            key("1", "S", "S/A", None, Some("X"))
        );
        assert_ne!(
            key("1", "S", "S/A/B", None, None),
            key("1", "S", "S/AB", None, None)
        );
    }

    #[test]
    fn preview_windows_progress_beyond_256_tracks_and_restart_after_short_page() {
        let mut script = Vec::new();
        for (window, response) in [
            ("0:128", page(0, 128, "Primeiro")),
            ("128:256", page(128, 128, "Álbum posterior")),
            ("256:384", page(256, 44, "Último")),
            ("0:128", page(0, 128, "Primeiro revisitado")),
        ] {
            script.push(("status".to_string(), ACTIVE.to_string()));
            script.push((format!("find \"(base 'Nova')\" window {window}"), response));
            script.push(("status".to_string(), ACTIVE.to_string()));
        }
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(script);
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview(&socket, &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        for (album, count, offset) in [
            ("Primeiro", 128, 128),
            ("Álbum posterior", 128, 256),
            ("Último", 44, 0),
            ("Primeiro revisitado", 128, 128),
        ] {
            let sample = read(&accepted.id, &lease).unwrap();
            assert_eq!(sample.sampled_tracks, count);
            assert_eq!(sample.albums.len(), 1); // Replaces, never accumulates.
            assert_eq!(sample.albums[0].album.as_deref(), Some(album));
            assert_eq!(lease.offsets().unwrap(), [offset]);
            assert!(sample.provisional && !sample.playable && !sample.complete);
        }
        server.join().unwrap();
    }

    #[test]
    fn preview_windows_cycle_over_growing_and_shrinking_partial_catalog() {
        let mut script = Vec::new();
        let observations = [(0, 128), (128, 0), (0, 100), (0, 128), (128, 112), (0, 0)];
        for (start, count) in observations {
            script.push(("status".to_string(), ACTIVE.to_string()));
            script.push((
                format!("find \"(base 'Nova')\" window {start}:{}", start + 128),
                page(start, count, "Mutável"),
            ));
            script.push(("status".to_string(), ACTIVE.to_string()));
        }
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(script);
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview(&socket, &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        for (start, count) in observations {
            let sample = read(&accepted.id, &lease).unwrap();
            assert_eq!(sample.sampled_tracks, count);
            assert_eq!(
                lease.offsets().unwrap(),
                [if count < 128 { 0 } else { start + 128 }]
            );
        }
        server.join().unwrap();
    }

    #[test]
    fn preview_cursor_does_not_advance_on_ack_invalid_partial_excessive_or_stale_read() {
        let mut script = vec![
            ("status".to_string(), ACTIVE.to_string()),
            (FIND.to_string(), page(0, 128, "Primeiro")),
            ("status".to_string(), ACTIVE.to_string()),
        ];
        for response in [
            "ACK [50@0] {find} unavailable\n".to_string(),
            "file: Old/a.flac\nOK\n".to_string(),
            "file: Nova/a.flac\n".to_string(),
            page(128, 129, "Excessivo"),
        ] {
            script.push(("status".into(), ACTIVE.into()));
            script.push(("find \"(base 'Nova')\" window 128:256".into(), response));
        }
        script.extend([
            ("status".into(), ACTIVE.into()),
            (
                "find \"(base 'Nova')\" window 128:256".into(),
                page(128, 2, "Obsoleto"),
            ),
            ("status".into(), "updating_db: 8\nOK\n".into()),
            ("status".into(), ACTIVE.into()),
            (
                "find \"(base 'Nova')\" window 128:256".into(),
                page(128, 2, "Válido"),
            ),
            ("status".into(), ACTIVE.into()),
        ]);
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(script);
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview(&socket, &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        read(&accepted.id, &lease).unwrap();
        for error in [
            PreviewError::ReadFailed,
            PreviewError::InvalidResponse,
            PreviewError::ReadFailed,
            PreviewError::LimitExceeded,
            PreviewError::Unavailable,
        ] {
            assert_eq!(read(&accepted.id, &lease).unwrap_err(), error);
            assert_eq!(lease.offsets().unwrap(), [128]);
        }
        assert_eq!(
            read(&accepted.id, &lease).unwrap().albums[0]
                .album
                .as_deref(),
            Some("Válido")
        );
        assert_eq!(lease.offsets().unwrap(), [0]);
        server.join().unwrap();
    }

    #[test]
    fn preview_job_replacement_resets_offsets_and_rejects_in_flight_commit() {
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let old_job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let mut next_job = None;
        let result = read_with(&accepted.id, &lease, |command, _| {
            if command == "status" && next_job.is_some() {
                return Ok(lines("updating_db: 7"));
            }
            if command.starts_with("find ") {
                next_job = Some(op.preview_job(8));
                return Ok(lines(&page(0, 128, "Obsoleto").trim_end_matches("OK\n")));
            }
            Ok(lines("updating_db: 7"))
        });
        assert!(matches!(result, Err(PreviewError::Unavailable)));
        assert_eq!(lease.offsets(), Err(PreviewError::Unavailable));
        drop(old_job);
        drop(lease);
        // Respect the unchanged admission cooldown when acquiring a fresh lease.
        std::thread::sleep(Duration::from_millis(500));
        let lease = state.reserve_preview(&accepted.id).unwrap();
        assert_eq!(lease.scope.job, Some(8));
        assert_eq!(lease.offsets().unwrap(), [0]);
        lease.commit_offsets(&[0], vec![128]).unwrap();
        state.request_shutdown(|| {});
        assert_eq!(
            lease.commit_offsets(&[128], vec![256]),
            Err(PreviewError::Unavailable)
        );
        drop(next_job);
    }

    #[test]
    fn preview_source_offsets_commit_together_and_cycle_independently() {
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview("unused", &["Nova".into(), "Outra".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let failed = read_with(&accepted.id, &lease, |command, _| {
            if command == "status" {
                return Ok(lines("updating_db: 7"));
            }
            if command.contains("'Nova'") {
                return Ok(lines(page(0, 64, "A").trim_end_matches("OK\n")));
            }
            Err(PreviewError::ReadFailed)
        });
        assert!(matches!(failed, Err(PreviewError::ReadFailed)));
        assert_eq!(lease.offsets().unwrap(), [0, 0]);
        read_with(&accepted.id, &lease, |command, _| {
            if command == "status" {
                Ok(lines("updating_db: 7"))
            } else if command.contains("'Nova'") {
                Ok(lines(page(0, 64, "A").trim_end_matches("OK\n")))
            } else {
                Ok(lines("file: Outra/a.flac\nAlbum: B"))
            }
        })
        .unwrap();
        assert_eq!(lease.offsets().unwrap(), [64, 0]);
    }

    #[test]
    fn preview_mutable_mpd_pages_replace_disappearing_uris_and_metadata_without_count() {
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
            ("status", ACTIVE),
            (FIND, "file: Nova/Album/image.flac\nAlbum: Físico\nArtist: A\nDisc: 1\nOK\n"),
            ("status", ACTIVE),
            ("status", ACTIVE),
            (FIND, "file: Nova/Album/disco.CUE/track0001\nAlbum: CUE\nAlbumArtist: B\nDisc: 2\nfile: Nova/Album/disco.CUE/track0002\nAlbum: CUE\nAlbumArtist: B\nDisc: 2\nOK\n"),
            ("status", ACTIVE),
        ]);
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview(&socket, &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let first = read(&accepted.id, &lease).unwrap();
        let next = read(&accepted.id, &lease).unwrap();
        assert_eq!(first.sampled_tracks, 1);
        assert_eq!(next.sampled_tracks, 2);
        assert_eq!(first.albums[0].album.as_deref(), Some("Físico"));
        assert_eq!(next.albums[0].album.as_deref(), Some("CUE"));
        assert_eq!(next.albums[0].discs, ["2"]);
        assert!(next.provisional && !next.playable && !next.complete);
        let json = serde_json::to_string(&next).unwrap();
        assert!(
            !json.contains("track000")
                && !json.contains("image.flac")
                && !json.contains("album_id")
        );
        // No merged old rows or assumed stable CUE/album identifiers.
        assert!(!json.contains("Físico"));
        drop(lease);
        drop(job);
        op.finish(Ok(()), false);
        server.join().unwrap();
    }

    #[test]
    fn preview_multiple_new_sources_use_individual_bounded_base_filters() {
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
            ("status", ACTIVE),
            (
                "find \"(base 'Nova')\" window 0:64",
                "file: Nova/a.flac\nAlbum: Mesmo título\nOK\n",
            ),
            (
                "find \"(base 'Áudio Novo')\" window 0:64",
                "file: Áudio Novo/b.flac\nAlbum: Mesmo título\nOK\n",
            ),
            ("status", ACTIVE),
        ]);
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview(&socket, &["Nova".into(), "Áudio Novo".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let page = read(&accepted.id, &lease).unwrap();
        assert_eq!(page.sampled_tracks, 2);
        assert_eq!(page.albums.len(), 2);
        assert_eq!(
            page.albums
                .iter()
                .map(|row| row.source_id.as_str())
                .collect::<Vec<_>>(),
            ["Nova", "Áudio Novo"]
        );
        server.join().unwrap();
    }

    #[test]
    fn preview_requires_backend_scope_active_job_and_matching_operation() {
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        assert!(matches!(
            state.reserve_preview(&accepted.id),
            Err(PreviewError::Unavailable)
        ));
        op.enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        assert!(matches!(
            state.reserve_preview(&accepted.id),
            Err(PreviewError::Unavailable)
        ));
        let job = op.preview_job(7);
        assert!(matches!(
            state.reserve_preview("arbitrary-operation"),
            Err(PreviewError::Unavailable)
        ));
        let lease = state.reserve_preview(&accepted.id).unwrap();
        assert!(matches!(
            state.reserve_preview(&accepted.id),
            Err(PreviewError::Busy)
        ));
        // Preview I/O never owns the transaction mutex; status remains queryable.
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Indexing);
        drop(job);
        assert_eq!(lease.ensure_active(), Err(PreviewError::Unavailable));
        drop(lease);
        op.finish(Ok(()), false);
        let (next, op) = state.reserve().unwrap();
        op.enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(8);
        assert!(matches!(
            state.reserve_preview(&accepted.id),
            Err(PreviewError::Unavailable)
        ));
        // Rate limit survives an operation replacement; no query burst.
        assert!(matches!(
            state.reserve_preview(&next.id),
            Err(PreviewError::Busy)
        ));
    }

    #[test]
    fn preview_invalidates_in_flight_on_completion_failure_uncertain_validation_and_shutdown() {
        for ending in 0..5 {
            let state = Arc::new(Coordinator::new());
            let (accepted, op) = state.reserve().unwrap();
            op.enable_addition_preview("unused", &["Nova".into()])
                .unwrap();
            op.phase(Phase::Indexing);
            let job = op.preview_job(7);
            let lease = state.reserve_preview(&accepted.id).unwrap();
            drop(job);
            // Retain an active job independently while triggering the end below.
            let job = op.preview_job(7);
            let mut op = Some(op);
            let mut queries = 0;
            let result = read_with(&accepted.id, &lease, |cmd, _| {
                queries += 1;
                if queries == 3 {
                    match ending {
                        0 => op.take().unwrap().finish(Ok(()), false),
                        1 => op.take().unwrap().finish(Err("failed".into()), false),
                        2 => op.take().unwrap().finish(Err("uncertain".into()), true),
                        3 => op.as_ref().unwrap().phase(Phase::Validating),
                        _ => state.request_shutdown(|| {}),
                    }
                }
                Ok(if cmd == "status" {
                    lines("updating_db: 7")
                } else {
                    lines("file: Nova/a.flac\nAlbum: A")
                })
            });
            assert!(matches!(result, Err(PreviewError::Unavailable)));
            drop(job);
        }
    }

    #[test]
    fn preview_invalid_sources_and_scope_escape_are_rejected() {
        for id in ["", "../Nova", "Nova/sub", "http://x", "Nova\nkill", "."] {
            assert!(Scope::new("unused", &[id.into()]).is_err());
        }
        assert!(Scope::new("unused", &["Nova".into(), "Nova".into()]).is_err());
        assert!(Scope::new(
            "unused",
            &(0..9).map(|n| format!("S{n}")).collect::<Vec<_>>()
        )
        .is_err());
        assert!(Scope::new("unused", &["x".repeat(257)]).is_err());
        let scope = Scope::new("unused", &["Áudio 'novo' \\".into(), "Outra".into()]).unwrap();
        assert_eq!(scope.filters.len(), 2);
        assert_eq!(
            scope.filters[0],
            AudioEngine::local_album_base_filter(&scope.sources[0]).unwrap()
        );
        for uri in [
            "Old/a.flac",
            "Nova/../a.flac",
            "Nova/http://a",
            "Nova/cover.jpg",
        ] {
            assert!(matches!(
                discoveries("1", lines(&format!("file: {uri}")), &["Nova".into()]),
                Err(PreviewError::InvalidResponse)
            ));
        }
    }

    #[test]
    fn preview_slow_read_does_not_lock_save_status_and_replacement_invalidates_result() {
        use std::sync::mpsc;
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let job = op.preview_job(7);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let (started, ready) = mpsc::channel();
        let (resume, wait) = mpsc::channel();
        let read_id = accepted.id.clone();
        let reader = std::thread::spawn(move || {
            read_with(&read_id, &lease, |cmd, _| {
                if cmd.starts_with("find ") {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(2)).unwrap();
                    Ok(lines("file: Nova/a.flac\nAlbum: A"))
                } else {
                    Ok(lines("updating_db: 7"))
                }
            })
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Indexing);
        assert!(matches!(
            state.reserve_preview(&accepted.id),
            Err(PreviewError::Busy)
        ));
        op.finish(Ok(()), false);
        let (next, next_op) = state.reserve().unwrap();
        next_op
            .enable_addition_preview("unused", &["Nova".into()])
            .unwrap();
        next_op.phase(Phase::Indexing);
        let next_job = next_op.preview_job(8);
        drop(job); // An old job guard cannot clear the newer operation's scope.
        assert!(matches!(
            state.reserve_preview(&next.id),
            Err(PreviewError::Busy)
        ));
        resume.send(()).unwrap();
        assert!(matches!(
            reader.join().unwrap(),
            Err(PreviewError::Unavailable)
        ));
        drop(next_job);
        next_op.finish(Ok(()), false);
    }

    #[test]
    fn preview_read_failures_status_completion_and_raw_errors_never_return_discoveries() {
        for (responses, expected) in [
            (
                vec![("status", "state: stop\nOK\n")],
                PreviewError::Unavailable,
            ),
            (
                vec![("status", "updating_db: 8\nOK\n")],
                PreviewError::Unavailable,
            ),
            (
                vec![
                    ("status", ACTIVE),
                    (
                        FIND,
                        "ACK [50@0] {find} https://private/?X-Plex-Token=secret\n",
                    ),
                ],
                PreviewError::ReadFailed,
            ),
            (
                vec![("status", ACTIVE), (FIND, "file: Nova/a.flac\n")],
                PreviewError::ReadFailed,
            ),
            (
                vec![("status", ACTIVE), (FIND, "file: Old/a.flac\nOK\n")],
                PreviewError::InvalidResponse,
            ),
            (
                vec![
                    ("status", ACTIVE),
                    (FIND, "file: Nova/a.flac\nOK\n"),
                    ("status", "state: stop\nOK\n"),
                ],
                PreviewError::Unavailable,
            ),
        ] {
            let (socket, server) = crate::tests::fake_restorative_rescan_mpd(responses);
            let state = Arc::new(Coordinator::new());
            let (accepted, op) = state.reserve().unwrap();
            op.enable_addition_preview(&socket, &["Nova".into()])
                .unwrap();
            op.phase(Phase::Indexing);
            let _job = op.preview_job(7);
            let lease = state.reserve_preview(&accepted.id).unwrap();
            assert_eq!(read(&accepted.id, &lease).unwrap_err(), expected);
            assert!(!serde_json::to_string(&expected).unwrap().contains("secret"));
            server.join().unwrap();
        }
    }

    #[test]
    fn preview_limits_tracks_fields_and_wire_responses() {
        let over = (0..=TRACK_LIMIT)
            .map(|n| format!("file: Nova/{n}.flac"))
            .collect();
        assert!(matches!(
            discoveries("1", over, &["Nova".into()]),
            Err(PreviewError::LimitExceeded)
        ));
        let response =
            lines("file: Nova/a.flac\nAlbum: https://private/?X-Plex-Token=secret\nArtist: Good");
        let (rows, _) = discoveries("1", response, &["Nova".into()]).unwrap();
        assert!(rows[0].album.is_none());
        assert_eq!(rows[0].artist.as_deref(), Some("Good"));
        assert!(display(&"x".repeat(257)).is_none());
        use std::io::Write;
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            // A single oversized line cannot allocate an unbounded String.
            peer.write_all(&vec![b'x'; LINE_LIMIT + 1]).unwrap();
        });
        assert_eq!(
            line(
                &mut BufReader::new(stream),
                Instant::now() + Duration::from_secs(2),
                &mut 0
            ),
            Err(PreviewError::LimitExceeded)
        );
        server.join().unwrap();
        let (stream, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"x\n").unwrap();
        let mut consumed = BYTE_LIMIT;
        assert_eq!(
            line(
                &mut BufReader::new(stream),
                Instant::now() + Duration::from_secs(2),
                &mut consumed
            ),
            Err(PreviewError::LimitExceeded)
        );
        let (stream, _peer) = UnixStream::pair().unwrap();
        assert_eq!(
            line(
                &mut BufReader::new(stream),
                Instant::now() + Duration::from_millis(20),
                &mut 0
            ),
            Err(PreviewError::ReadFailed)
        );
    }
}
