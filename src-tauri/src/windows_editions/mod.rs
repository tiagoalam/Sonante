//! Windows adapter for the original read-only album edition analysis.
pub(crate) mod policy;
use crate::windows_mpd::library::Library;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
pub(crate) fn local(
    library: Library,
    album_id: String,
    scope: (u64, crate::windows_mpd::library::Sources),
) -> Result<Option<policy::Profile>, String> {
    ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "EDITIONS_SOURCE")?;
    let permit = Permit;
    crate::windows_io::read(Duration::from_secs(3), move |job| {
        let _permit = permit;
        let start = Instant::now();
        let token = library
            .metadata_snapshot_token()
            .map_err(|_| "EDITIONS_SOURCE")?;
        let current = || {
            job.current()
                && start.elapsed() < Duration::from_secs(2)
                && library.edition_scope().ok() == Some(scope)
                && library.check_metadata_snapshot(&token).is_ok()
        };
        if !scope.1.local || !current() {
            return Err("EDITIONS_SOURCE");
        }
        let mut tracks = library
            .metadata_members(&album_id, &token)
            .map_err(|_| "EDITIONS_SOURCE")?
            .ok_or("EDITIONS_SOURCE")?;
        tracks.sort_by_cached_key(|t| (t.disc, t.number, t.file_name.to_lowercase()));
        let uris = tracks
            .iter()
            .map(crate::windows_mpd::library::track_uri)
            .collect::<Vec<_>>();
        let sizes = if uris
            .iter()
            .any(|u| crate::windows_metadata::is_virtual_cue_track_uri(u))
        {
            None
        } else {
            crate::windows_file_info::catalog_sizes(&library, &uris, &current).ok()
        };
        if !current() {
            return Err("EDITIONS_SOURCE");
        }
        Ok(Some(policy::Profile::new(
            sizes,
            tracks.into_iter().map(|t| Some(t.title)),
        )))
    })
    .map_err(|_| "EDITIONS_SOURCE")?
    .map_err(str::to_owned)
}

#[cfg(test)]
mod tests;
