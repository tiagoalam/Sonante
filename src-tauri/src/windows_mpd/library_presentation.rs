//! Queue metadata only. No new reads, playback clock, durable fields or authority.
use super::*;
pub(crate) fn duration(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v > 0.0)
}
impl Library {
    pub(crate) fn queue_durations(&self, queue: &[Track]) -> Vec<Option<f64>> {
        if queue.len() > 1000 { return Vec::new(); }
        // Catalog projection/scan must not delay the serialized audio worker.
        // Busy/poisoned/absent metadata stays unknown; playback is unaffected.
        let state = self.data.try_lock().ok();
        queue.iter().map(|track| {
            if track.radio.is_some() { return None; }
            if let Some(source) = &track.remote_source {
                let lease = track.remote_lease.as_ref()?;
                return if lease.source() == source && lease.healthy() {
                    duration(lease.presentation_duration())
                } else { None };
            }
            let state = state.as_ref()?;
            let record = state.albums.get(track.catalog_album_id.as_ref()?)?;
            if track.media_root.as_ref() != Some(&record.root)
                || track.id != id(&track.path.to_string_lossy()) { return None; }
            duration(record.durations.get(&track.id).copied())
        }).collect()
    }
}

#[cfg(test)]
#[path="library_presentation_tests.rs"]
mod tests;
