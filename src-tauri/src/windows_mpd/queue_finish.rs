//! Complete an already proven, stopped, owned MPD queue deletion.
//! No engine mutation/replay here: only persist its confirmed empty state and
//! release the owned route after successful cleanup. Failed cleanup retains it.
use super::{protocol::Result, queue::Journal};
pub(super) fn confirmed_empty<T>(
    journal: &mut Journal,
    current: &mut Option<T>,
    close: impl FnOnce(&mut T) -> Result<()>,
) -> Result<()> {
    let route = current.as_mut().ok_or("no_track")?;
    journal.observe_empty();
    close(route)?;
    drop(current.take());
    if journal.warning.is_some() {
        Err("PLAYBACK_PERSISTENCE_FAILED")
    } else {
        Ok(())
    }
}
#[cfg(test)]
#[path = "queue_finish_tests.rs"]
mod tests;
