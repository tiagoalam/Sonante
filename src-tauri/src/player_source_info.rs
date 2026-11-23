//! Read-only source facts. No MPD requests, transport mutation or persistent cache.
use crate::{audio::{AudioState, CurrentMedia, MediaLocator, TrackMetadata}, ConfigState,
    local_file_info::{InspectionPermit, LocalFileInfo}, PlexState, supervisor::MpdSupervisor, tags};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, SemaphorePermit};
use tauri::State;

const MAX_LRC_BYTES: usize = 512 * 1024;

#[derive(Debug, Serialize)]
pub struct SourceAudioFacts {
    pub provenance: &'static str,
    pub codec: &'static str,
    pub compression: &'static str,
    pub sample_type: &'static str,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub dsd_level: Option<u16>,
    pub revision: Option<String>,
}
impl From<LocalFileInfo> for SourceAudioFacts {
    fn from(info: LocalFileInfo) -> Self {
        Self { provenance: "local", codec: info.codec, compression: info.compression,
            sample_type: info.sample_type, sample_rate: info.sample_rate,
            bit_depth: info.bit_depth, dsd_level: info.dsd_level, revision: Some(info.revision) }
    }
}
#[derive(Serialize)]
pub struct PlayerSourceInfo { media: CurrentMedia, info: SourceAudioFacts }

// One physical acquisition globally, including the worker after a caller disappears.
// Waiters queue here rather than turning a harmless overlap into a user-visible
// read failure. The permit is moved into blocking workers and released on Drop.
static SOURCE_READ_ADMISSION: Semaphore = Semaphore::const_new(1);
pub(crate) struct SourcePermit {
    _permit: SemaphorePermit<'static>,
}
impl SourcePermit {
    pub(crate) async fn acquire() -> Option<Self> {
        SOURCE_READ_ADMISSION.acquire().await.ok().map(|permit| Self { _permit: permit })
    }
}

fn queued_locator(queue: &[TrackMetadata], media: &CurrentMedia) -> Option<MediaLocator> {
    match media {
        CurrentMedia::Upnp { .. } | CurrentMedia::Radio { .. } | CurrentMedia::Navidrome { .. } => None,
        CurrentMedia::Local { uri, queue_index } => {
            let track = queue.get(*queue_index)?;
            match &track.media_locator {
                Some(MediaLocator::Local { uri: queued }) if queued == uri => Some(MediaLocator::Local { uri: queued.clone() }),
                // Legacy Local only. Never accept Plex mapped paths as Local.
                None if track.uri == *uri && !uri.is_empty() => Some(MediaLocator::Local { uri: uri.clone() }),
                _ => None,
            }
        }
        CurrentMedia::Plex { .. } => crate::queued_plex_navigation_locator(queue, media).ok(),
    }
}
fn snapshot(audio: &State<'_, AudioState>, media: &CurrentMedia) -> Option<MediaLocator> {
    queued_locator(&audio.0.lock().ok()?.get_queue(), media)
}

async fn acquire_player_source_info(media: CurrentMedia, audio_state: State<'_, AudioState>,
    config_state: State<'_, ConfigState>, state: State<'_, PlexState>,
    sqlite_library: State<'_, crate::sqlite_library::SqliteLibraryState>) -> Option<PlayerSourceInfo> {
    let permit = SourcePermit::acquire().await?;
    let locator = snapshot(&audio_state, &media)?;
    let info = match &locator {
        MediaLocator::Upnp { .. } | MediaLocator::Radio { .. } | MediaLocator::Navidrome { .. } => return None,
        MediaLocator::Local { uri } if crate::sqlite_library::SqliteLibraryState::is_sqlite_track_locator(uri) => {
            let _permit = permit;
            sqlite_library.player_source_facts(uri).ok()??
        }
        MediaLocator::Local { uri } => {
            let file_permit = InspectionPermit::acquire().ok()?;
            let folders = config_state.0.lock().ok()?.local_folders.clone();
            let expected = folders.clone();
            let uri = uri.clone();
            // All file work uses the existing bounded reader, root resolver and revision checks.
            let info = tauri::async_runtime::spawn_blocking(move || {
                let (_source_permit, _file_permit) = (permit, file_permit);
                tags::inspect_local_file_info(&MpdSupervisor::library_dir(), &uri, &folders)
            }).await.ok()?.ok()?;
            if config_state.0.lock().ok()?.local_folders != expected { return None; }
            SourceAudioFacts::from(info)
        }
        MediaLocator::Plex { .. } => {
            let _permit = permit;
            let client = state.0.lock().ok()?.clone();
            let revision = client.source_revision()?;
            let info = client.source_audio_facts(&locator).await?;
            if !state.0.lock().ok()?.source_context_matches(&client, revision) { return None; }
            info
        }
    };
    if snapshot(&audio_state, &media).as_ref() != Some(&locator) { return None; }
    Some(PlayerSourceInfo { media, info })
}

#[tauri::command]
pub async fn get_player_source_info(media: CurrentMedia, audio_state: State<'_, AudioState>,
    config_state: State<'_, ConfigState>, state: State<'_, PlexState>,
    sqlite_library: State<'_, crate::sqlite_library::SqliteLibraryState>) -> Result<Option<PlayerSourceInfo>, ()> {
    Ok(acquire_player_source_info(media, audio_state, config_state, state, sqlite_library).await)
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalLyricsLookup {
    Found { content: String },
    NotFound,
    NotEligible { reason: &'static str },
    ReadFailed { reason: &'static str },
    InvalidContent { reason: &'static str },
}

fn confirm_local_queue_identity(
    current: Option<&CurrentMedia>,
    queue: &[TrackMetadata],
    requested: &CurrentMedia,
) -> Result<MediaLocator, &'static str> {
    if current != Some(requested) {
        return Err("stale");
    }
    let locator = queued_locator(queue, requested).ok_or("stale")?;
    if !matches!(locator, MediaLocator::Local { .. }) {
        return Err("not_local");
    }
    Ok(locator)
}

fn confirm_current_local_locator(
    audio: &AudioState,
    requested: &CurrentMedia,
) -> Result<MediaLocator, &'static str> {
    let engine = audio.0.lock().map_err(|_| "unavailable")?;
    let status = engine.get_telemetry_status().map_err(|_| "unavailable")?;
    confirm_local_queue_identity(
        status.current_media.as_ref(),
        &engine.get_queue(),
        requested,
    )
}

fn decode_local_lyrics(bytes: Vec<u8>) -> LocalLyricsLookup {
    if bytes.len() > MAX_LRC_BYTES {
        return LocalLyricsLookup::InvalidContent {
            reason: "too_large",
        };
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let Ok(content) = std::str::from_utf8(bytes) else {
        return LocalLyricsLookup::InvalidContent {
            reason: "invalid_utf8",
        };
    };
    if content.contains('\0') {
        return LocalLyricsLookup::InvalidContent {
            reason: "invalid_text",
        };
    }
    LocalLyricsLookup::Found {
        content: content.to_owned(),
    }
}

fn is_eligible_physical_local_uri(uri: &str) -> bool {
    if crate::sqlite_library::SqliteLibraryState::is_sqlite_cue_locator(uri) {
        return false;
    }
    if crate::sqlite_library::SqliteLibraryState::is_sqlite_track_locator(uri) {
        return true;
    }
    !uri.contains("://") && !crate::tags::is_virtual_cue_track(uri)
}

/// Read the exact LRC sidecar for the currently playing physical Local track.
/// Current-media and queue identity are checked before and after bounded disk
/// I/O; the command never accepts a path/locator directly from the caller.
#[tauri::command]
pub async fn get_current_local_lyrics(
    media: CurrentMedia,
    audio_state: State<'_, AudioState>,
    config_state: State<'_, ConfigState>,
    sqlite_library: State<'_, crate::sqlite_library::SqliteLibraryState>,
) -> Result<LocalLyricsLookup, ()> {
    let Some(permit) = SourcePermit::acquire().await else {
        return Ok(LocalLyricsLookup::ReadFailed { reason: "busy" });
    };
    let locator = match confirm_current_local_locator(&audio_state, &media) {
        Ok(locator) => locator,
        Err("not_local") => {
            return Ok(LocalLyricsLookup::NotEligible {
                reason: "not_local",
            })
        }
        Err("stale") => {
            return Ok(LocalLyricsLookup::ReadFailed {
                reason: "stale_track",
            })
        }
        Err(_) => {
            return Ok(LocalLyricsLookup::ReadFailed {
                reason: "unavailable",
            })
        }
    };
    let uri = match locator {
        MediaLocator::Local { uri } => uri,
        _ => {
            return Ok(LocalLyricsLookup::NotEligible {
                reason: "not_local",
            })
        }
    };
    if !is_eligible_physical_local_uri(&uri) {
        return Ok(LocalLyricsLookup::NotEligible {
            reason: "cue_track",
        });
    }

    let roots = config_state.0.lock().map_err(|_| ())?.local_folders.clone();
    let roots_for_read = roots.clone();
    let sqlite = (*sqlite_library).clone();
    let library_dir = MpdSupervisor::library_dir();
    let requested_uri = uri.clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        // The command future may be cancelled while spawn_blocking continues.
        // Keep admission owned by the worker until its filesystem read ends.
        let _permit = permit;
        if crate::sqlite_library::SqliteLibraryState::is_sqlite_track_locator(&requested_uri) {
            sqlite
                .read_local_track_lrc(&requested_uri, MAX_LRC_BYTES)
                .map_err(|error| match error.as_str() {
                    "SQLITE_LYRICS_TOO_LARGE" => crate::tags::LocalLyricsReadError::TooLarge,
                    "SQLITE_LYRICS_PERMISSION_DENIED" => {
                        crate::tags::LocalLyricsReadError::PermissionDenied
                    }
                    "SQLITE_LYRICS_TRACK_NOT_ELIGIBLE" => {
                        crate::tags::LocalLyricsReadError::NotEligible
                    }
                    _ => crate::tags::LocalLyricsReadError::Changed,
                })
        } else {
            crate::tags::read_local_track_lrc(
                &library_dir,
                &requested_uri,
                &roots_for_read,
                MAX_LRC_BYTES,
            )
        }
    })
    .await
    .map_err(|_| ())?;
    let lookup = match bytes {
        Ok(Some(bytes)) => decode_local_lyrics(bytes),
        Ok(None) => LocalLyricsLookup::NotFound,
        Err(crate::tags::LocalLyricsReadError::NotEligible) => LocalLyricsLookup::NotEligible {
            reason: "not_local_track",
        },
        Err(crate::tags::LocalLyricsReadError::TooLarge) => LocalLyricsLookup::InvalidContent {
            reason: "too_large",
        },
        Err(crate::tags::LocalLyricsReadError::PermissionDenied) => LocalLyricsLookup::ReadFailed {
            reason: "permission_denied",
        },
        Err(crate::tags::LocalLyricsReadError::Changed) => LocalLyricsLookup::ReadFailed {
            reason: "source_changed",
        },
        Err(crate::tags::LocalLyricsReadError::ReadFailed) => LocalLyricsLookup::ReadFailed {
            reason: "read_failed",
        },
    };
    if config_state.0.lock().map_err(|_| ())?.local_folders != roots {
        return Ok(LocalLyricsLookup::ReadFailed {
            reason: "source_changed",
        });
    }
    match confirm_current_local_locator(&audio_state, &media) {
        Ok(current) if current == (MediaLocator::Local { uri }) => Ok(lookup),
        _ => Ok(LocalLyricsLookup::ReadFailed {
            reason: "stale_track",
        }),
    }
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceAlbum {
    Local { album_id: String, track_count: usize },
    Plex { server_id: String, library_id: String, rating_key: String },
}
// Only fixed categories/counts, compiled out of release builds. No media values.
pub(crate) fn album_source_absent<T>(stage: &'static str, reason: &'static str, count: usize) -> Option<T> {
    #[cfg(debug_assertions)]
    eprintln!("[Sonante] dev.album-source stage={stage} reason={reason} tracks={}", count.min(33));
    #[cfg(not(debug_assertions))] let _ = (stage, reason, count);
    None
}
fn album_file_reason(error: crate::local_file_info::InfoError) -> &'static str {
    use crate::local_file_info::InfoError::*;
    match error { InvalidLocalPath => "invalid_path", OutsideConfiguredRoots => "outside_roots", FileNotFound => "missing",
        CueVirtual => "cue_virtual", UnsupportedFormat => "unsupported_format", CorruptHeader => "corrupt_header",
        PermissionDenied => "permission_denied", Unavailable => "unavailable", Stale => "stale", Busy => "busy" }
}
async fn acquire_album_source_info(album: SourceAlbum, audio: State<'_, AudioState>,
    config: State<'_, ConfigState>, plex: State<'_, PlexState>) -> Option<Vec<SourceAudioFacts>> {
    let permit = SourcePermit::acquire().await.or_else(|| album_source_absent("admission", "busy", 0))?;
    match album {
        SourceAlbum::Local { album_id, track_count } => {
            if track_count == 0 || track_count > 32 || album_id.len() > 4096 { return album_source_absent("identity", "limit", track_count); }
            let file_permit = InspectionPermit::acquire().ok().or_else(|| album_source_absent("file_admission", "busy", 0))?;
            let socket = audio.0.lock().ok()?.local_album_socket_path();
            let folders = config.0.lock().ok()?.local_folders.clone();
            let expected = folders.clone();
            let result = tauri::async_runtime::spawn_blocking(move || {
                let (_source_permit, _file_permit) = (permit, file_permit);
                let started = std::time::Instant::now();
                let groups = match crate::audio::AudioEngine::get_source_album_tracks_at(&socket, &album_id) {
                    Ok(Some(groups)) => groups,
                    Ok(None) => return album_source_absent("local_index", "not_found", 0),
                    Err(error) => return album_source_absent("local_index", if error == "SOURCE_ALBUM_LIMIT" { "limit" } else { "unavailable" }, 0),
                };
                let tracks: Vec<_> = groups.iter().flat_map(|g| &g.tracks).collect();
                if tracks.len() != track_count || tracks.iter().any(|t| t.item_type != "file") { return album_source_absent("local_membership", "count_mismatch", tracks.len()); }
                let mut seen = std::collections::HashSet::new();
                let library = MpdSupervisor::library_dir();
                let mut result = Vec::with_capacity(track_count);
                for track in &tracks {
                    if started.elapsed() > std::time::Duration::from_secs(2) { return album_source_absent("local_budget", "deadline", result.len()); }
                    if !seen.insert(&track.path) { return album_source_absent("local_membership", "duplicate", result.len()); }
                    let info = match tags::inspect_local_file_info(&library, &track.path, &folders) {
                        Ok(info) => info,
                        Err(error) => return album_source_absent("local_header", album_file_reason(error), result.len()),
                    };
                    result.push(SourceAudioFacts::from(info));
                }
                for (track, info) in tracks.iter().zip(&result) {
                    if !tags::source_revision_matches(&library, &track.path, &folders, info.revision.as_deref()?) { return album_source_absent("local_revision", "stale", result.len()); }
                }
                // The index may have changed during disk reads; verify exact track membership again.
                let current = crate::audio::AudioEngine::get_source_album_tracks_at(&socket, &album_id).ok().flatten().or_else(|| album_source_absent("local_revalidate", "unavailable", result.len()))?;
                let current: std::collections::HashSet<_> = current.iter().flat_map(|g| g.tracks.iter().map(|t| &t.path)).collect();
                if current != seen { return album_source_absent("local_membership", "stale", result.len()); }
                if started.elapsed() > std::time::Duration::from_secs(2) { return album_source_absent("local_budget", "deadline", result.len()); }
                Some(result)
            }).await.ok()??;
            if config.0.lock().ok()?.local_folders != expected { return album_source_absent("context", "roots_changed", 0); }
            Some(result)
        }
        SourceAlbum::Plex { server_id, library_id, rating_key } => {
            let _permit = permit;
            let client = plex.0.lock().ok()?.clone();
            let revision = client.source_revision()?;
            let info = client.album_source_audio_facts(&server_id, &library_id, &rating_key).await.or_else(|| album_source_absent("plex_read_or_parse", "unavailable", 0))?;
            if !plex.0.lock().ok()?.source_context_matches(&client, revision) { return None; }
            Some(info)
        }
    }
}
#[tauri::command]
pub async fn get_album_source_info(album: SourceAlbum, audio: State<'_, AudioState>,
    config: State<'_, ConfigState>, plex: State<'_, PlexState>) -> Result<Option<Vec<SourceAudioFacts>>, ()> {
    let source = match &album { SourceAlbum::Local { .. } => "local", SourceAlbum::Plex { .. } => "plex" };
    let result = acquire_album_source_info(album, audio, config, plex).await;
    #[cfg(debug_assertions)] eprintln!("[Sonante] dev.album-source stage=backend_result source={source} available={} tracks={}", result.is_some(), result.as_ref().map_or(0, |v| v.len()));
    #[cfg(not(debug_assertions))] let _ = source;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(locator: Option<MediaLocator>, uri: &str) -> TrackMetadata {
        TrackMetadata { title: "fixture".into(), artist: "fixture".into(), album: "fixture".into(),
            thumb: None, plex_image: None, uri: uri.into(), duration: None, media_locator: locator }
    }
    #[test]
    fn source_queue_binding_rejects_frontend_paths_mapped_plex_and_wrong_occurrences() {
        let local = MediaLocator::Local { uri: "Music/a.flac".into() };
        let plex = MediaLocator::Plex { server_id: "server".into(), part_key: "/library/parts/1/file".into(), rating_key: Some("42".into()), file_path: None };
        let queue = vec![track(Some(local.clone()), "ignored"), track(Some(plex.clone()), "Music/a.flac"), track(None, "Music/legacy.flac")];
        let media = |uri: &str, queue_index| CurrentMedia::Local { uri: uri.into(), queue_index };
        assert_eq!(queued_locator(&queue, &media("Music/a.flac", 0)), Some(local));
        assert!(queued_locator(&queue, &media("Music/a.flac", 1)).is_none());
        assert!(queued_locator(&queue, &media("/arbitrary", 0)).is_none());
        assert!(queued_locator(&queue, &media("Music/a.flac", 3)).is_none());
        assert_eq!(queued_locator(&queue, &media("Music/legacy.flac", 2)), Some(MediaLocator::Local { uri: "Music/legacy.flac".into() }));
        let p = CurrentMedia::Plex { server_id: "server".into(), part_key: "/library/parts/1/file".into(), queue_index: 1 };
        assert_eq!(queued_locator(&queue, &p), Some(plex));
        let wrong = CurrentMedia::Plex { server_id: "other".into(), part_key: "/library/parts/1/file".into(), queue_index: 1 };
        assert!(queued_locator(&queue, &wrong).is_none());
    }
    #[test]
    fn local_lyrics_confirmation_rejects_stale_media_and_changed_queue_slot() {
        let requested = CurrentMedia::Local {
            uri: "sqlite-track://1/track.flac".into(),
            queue_index: 0,
        };
        let other = CurrentMedia::Local {
            uri: "sqlite-track://1/other.flac".into(),
            queue_index: 0,
        };
        let queue = vec![track(
            Some(MediaLocator::Local {
                uri: "sqlite-track://1/track.flac".into(),
            }),
            "ignored",
        )];
        assert!(confirm_local_queue_identity(None, &queue, &requested).is_err());
        assert_eq!(
            confirm_local_queue_identity(Some(&other), &queue, &requested),
            Err("stale")
        );
        assert!(confirm_local_queue_identity(Some(&requested), &[], &requested).is_err());
        assert_eq!(
            confirm_local_queue_identity(Some(&requested), &queue, &requested),
            Ok(MediaLocator::Local {
                uri: "sqlite-track://1/track.flac".into()
            })
        );
    }
    #[test]
    fn local_lyrics_excludes_cue_and_protocol_locators() {
        assert!(!is_eligible_physical_local_uri("sqlite-cue://1/YQ/1"));
        assert!(!is_eligible_physical_local_uri(
            "sonante-cue-range://encoded/0/open"
        ));
        assert!(!is_eligible_physical_local_uri("Album/Disc.cue/track01"));
        assert!(!is_eligible_physical_local_uri(
            "https://example.invalid/audio.flac"
        ));
        assert!(is_eligible_physical_local_uri("sqlite-track://1/YQ"));
        assert!(is_eligible_physical_local_uri("Music/01 - Airbag.flac"));
    }
    #[tokio::test]
    async fn source_admission_serializes_waiters_and_releases_after_the_owner() {
        let permit = SourcePermit::acquire().await.unwrap();
        let waiter = tokio::spawn(async { SourcePermit::acquire().await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "concurrent source reads wait instead of failing as busy");
        drop(permit);
        let next = waiter.await.unwrap();
        drop(next);
        assert!(SourcePermit::acquire().await.is_some());
    }
    #[test]
    fn local_lyrics_bom_utf8_and_content_limits_are_classified_without_partial_text() {
        assert_eq!(
            decode_local_lyrics(b"\xef\xbb\xbf[00:01.00]Synthetic".to_vec()),
            LocalLyricsLookup::Found {
                content: "[00:01.00]Synthetic".into()
            }
        );
        assert_eq!(
            decode_local_lyrics(vec![0xff, 0xfe]),
            LocalLyricsLookup::InvalidContent {
                reason: "invalid_utf8"
            }
        );
        assert_eq!(
            decode_local_lyrics(vec![0; MAX_LRC_BYTES + 1]),
            LocalLyricsLookup::InvalidContent {
                reason: "too_large"
            }
        );
        assert_eq!(
            decode_local_lyrics(b"line\0break".to_vec()),
            LocalLyricsLookup::InvalidContent {
                reason: "invalid_text"
            }
        );
    }
    #[test]
    fn source_info_local_fixture_uses_the_existing_bounded_flac_reader() {
        let bytes = crate::local_file_info::tests::fixture(96000, 24, 2, 96000);
        let mut cursor = std::io::Cursor::new(&bytes);
        let info = crate::local_file_info::read_file_info(&mut cursor, bytes.len() as u64).unwrap();
        let facts = SourceAudioFacts::from(info);
        assert_eq!(facts.codec, "FLAC"); assert_eq!(facts.sample_rate, Some(96000));
        println!("fixture_local_facts={}", serde_json::to_string(&vec![facts]).unwrap());
    }

}
