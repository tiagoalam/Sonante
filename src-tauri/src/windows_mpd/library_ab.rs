//! Windows source adapter for the UNCHANGED shared offline A/B sample core.
//! Catalog identities stay opaque; no physical path is accepted from IPC.
use super::*;
use crate::windows_shared::ab_comparison as ab;
use std::io::{self, SeekFrom};
use symphonia::core::io::MediaSource;
pub(crate) struct AbInventory {
    pub scope: (u64, Sources),
    pub snapshot: String,
    pub rows: Vec<(ab::Track, Track)>,
}
impl Library {
    /// Polling validity must never wait for a catalog/configuration lock or I/O.
    pub(crate) fn ab_scope_current(&self, scope: (u64, Sources), snapshot: Option<&str>) -> bool {
        let Ok(_change) = self.change.try_lock() else {
            return false;
        };
        let Ok(state) = self.data.try_lock() else {
            return false;
        };
        !self.stopping.load(Ordering::Acquire)
            && state.status.error != Some("library_settings")
            && !state.first_run
            && state.usage_mode == UsageMode::Full
            && state.experimental_features
            && (state.editions_revision, state.sources) == scope
            && snapshot.is_none_or(|token| {
                self.snapshot_token(&state)
                    .is_ok_and(|actual| actual == token)
            })
    }
    pub(crate) fn ab_storage_parent(&self) -> std::result::Result<PathBuf, String> {
        self.config
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "AB_STORAGE".into())
    }

    pub(crate) fn ab_inventory(&self, album: &str) -> std::result::Result<AbInventory, String> {
        if album.len() != 64
            || !album
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err("AB_SOURCE".into());
        }
        let scope = self.edition_scope().map_err(|_| "AB_CHANGED")?;
        if !scope.1.local {
            return Err("AB_SOURCE".into());
        }
        let (snapshot, rows) = {
            let state = self.data.lock().map_err(|_| "AB_UNAVAILABLE")?;
            let snapshot = self.snapshot_token(&state).map_err(|_| "AB_CHANGED")?;
            let record = state.albums.get(album).ok_or("AB_TRACKS")?;
            if record.tracks.is_empty() || record.tracks.len() > 500 {
                return Err("AB_TRACKS".into());
            }
            let mut tracks = record.tracks.clone();
            tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
            let rows = tracks
                .into_iter()
                .enumerate()
                .map(|(i, track)| {
                    let uri = track_uri(&track);
                    if track.radio.is_some()
                        || track.remote_source.is_some()
                        || track.catalog_album_id.as_deref() != Some(album)
                        || track.media_root.as_ref() != Some(&record.root)
                        || !track.path.starts_with(&record.root)
                        || crate::windows_metadata::is_virtual_cue_track_uri(&uri)
                    {
                        return Err("AB_SOURCE".into());
                    }
                    let item = ab::track(
                        &uri,
                        &track.title,
                        record.durations.get(&track.id).copied(),
                        i + 1,
                    )?;
                    Ok((item, track))
                })
                .collect::<std::result::Result<Vec<_>, String>>()?;
            (snapshot, rows)
        };
        if self.edition_scope().ok() != Some(scope)
            || self.check_metadata_snapshot(&snapshot).is_err()
        {
            return Err("AB_CHANGED".into());
        }
        Ok(AbInventory {
            scope,
            snapshot,
            rows,
        })
    }
    /// Must run on the caller's owned bounded loader worker. Returned closures
    /// retain no I/O-executor Job and no strong reference to their own AB Job.
    pub(crate) fn ab_local_input(
        &self,
        album: &str,
        key: &str,
        policy: ab::Check,
    ) -> std::result::Result<ab::Input, String> {
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("AB_SOURCE".into());
        }
        let inventory = self.ab_inventory(album)?;
        let track = inventory
            .rows
            .into_iter()
            .find_map(|(item, track)| (item.key == key).then_some(track))
            .ok_or("AB_CHANGED")?;
        let library = self.clone();
        let scope = inventory.scope;
        let snapshot = inventory.snapshot;
        let alive: ab::Check =
            Arc::new(move || policy() && library.ab_scope_current(scope, Some(&snapshot)));
        if !alive() {
            return Err("AB_CHANGED".into());
        }
        let root = track.media_root.as_ref().ok_or("AB_SOURCE")?;
        let content =
            crate::windows_file_info::RetainedContent::capture(root, &track.path, || alive())
                .map_err(|_| "AB_SOURCE")?;
        if content.len() == 0 || !alive() {
            return Err("AB_CHANGED".into());
        }
        let retained = content.clone();
        let check = alive.clone();
        let validate: ab::Check =
            Arc::new(move || check() && retained.validate(|| check()).is_ok() && check());
        let media = LocalMedia {
            content,
            current: alive.clone(),
        };
        Ok(ab::Input {
            media: Box::new(media),
            alive,
            validate,
            identity: format!("local:{key}"),
        })
    }
}
struct LocalMedia {
    content: Arc<crate::windows_file_info::RetainedContent>,
    current: ab::Check,
}
impl LocalMedia {
    fn allowed(&self) -> io::Result<()> {
        if (self.current)() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::Interrupted, "AB_CHANGED"))
        }
    }
}
impl io::Read for LocalMedia {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.allowed()?;
        let n = self.content.read(bytes)?;
        self.allowed()?;
        Ok(n)
    }
}
impl io::Seek for LocalMedia {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.allowed()?;
        let at = self.content.seek(position)?;
        self.allowed()?;
        Ok(at)
    }
}
impl MediaSource for LocalMedia {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.content.len())
    }
}
#[cfg(test)]
#[path = "library_ab_tests.rs"]
mod tests;
