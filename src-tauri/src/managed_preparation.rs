//! Dormant admission/fencing for a future NFS preparation worker. The live
//! managed-mirror paths still prepare under ConfigTransactionState. Activation
//! requires wiring revision bumps at every config/mirror mutation and using
//! the validate-then-publish wrapper from local_media without an intervening
//! shutdown window. The watcher shutdown still joins a potentially NFS-blocked
//! callback, and publication validation still touches physical sources. In
//! particular, a lease check alone is not a publish permit.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparationRequest {
    Automatic,
    Explicit,
}

pub(crate) enum PreparationAdmission<'a> {
    Started(PreparationLease<'a>),
    Coalesced,
    Busy,
    ShuttingDown,
}

#[derive(Default)]
struct PreparationState {
    active: Option<u64>,
    next_attempt: u64,
    config_revision: u64,
    generation_revision: u64,
    pending_automatic: bool,
    shutting_down: bool,
}

pub(crate) struct ManagedPreparationCoordinator {
    state: Mutex<PreparationState>,
}

pub(crate) struct PreparationLease<'a> {
    coordinator: &'a ManagedPreparationCoordinator,
    attempt: u64,
    config_revision: u64,
    generation_revision: u64,
    staging_path: PathBuf,
    released: bool,
}

impl ManagedPreparationCoordinator {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(PreparationState::default()),
        }
    }

    pub(crate) fn begin(
        &self,
        request: PreparationRequest,
        live_path: &Path,
    ) -> PreparationAdmission<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.shutting_down || state.next_attempt == u64::MAX {
            return PreparationAdmission::ShuttingDown;
        }
        if state.active.is_some() {
            return match request {
                PreparationRequest::Automatic => {
                    state.pending_automatic = true;
                    PreparationAdmission::Coalesced
                }
                PreparationRequest::Explicit => PreparationAdmission::Busy,
            };
        }
        state.next_attempt += 1;
        let attempt = state.next_attempt;
        state.active = Some(attempt);
        PreparationAdmission::Started(PreparationLease {
            coordinator: self,
            attempt,
            config_revision: state.config_revision,
            generation_revision: state.generation_revision,
            // The builder refuses an existing path; no previous staging is overwritten.
            staging_path: live_path.with_file_name(format!(
                "library.next.preparing-{}-{attempt}",
                std::process::id()
            )),
            released: false,
        })
    }

    pub(crate) fn note_config_change(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.config_revision.checked_add(1) {
            Some(next) => state.config_revision = next,
            None => state.shutting_down = true,
        }
    }

    pub(crate) fn note_generation_change(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.generation_revision.checked_add(1) {
            Some(next) => state.generation_revision = next,
            None => state.shutting_down = true,
        }
    }

    pub(crate) fn begin_shutdown_with(&self, mark_shutdown: impl FnOnce()) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.shutting_down = true;
        state.pending_automatic = false;
        // Admission and the lifecycle marker become one ordered transition.
        mark_shutdown();
    }
}

impl PreparationLease<'_> {
    pub(crate) fn staging_path(&self) -> &Path {
        &self.staging_path
    }

    /// Must be checked while holding ConfigTransactionState, immediately
    /// before the existing validate-then-publish sequence. It is not a permit
    /// to separate those two phases or to start an MPD job later.
    pub(crate) fn is_current(&self) -> bool {
        let state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        !state.shutting_down
            && state.active == Some(self.attempt)
            && state.config_revision == self.config_revision
            && state.generation_revision == self.generation_revision
    }

    /// The caller must schedule at most one further automatic pass if true.
    pub(crate) fn finish(mut self) -> bool {
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.released = true;
        if state.active != Some(self.attempt) {
            return false;
        }
        state.active = None;
        let pending = state.pending_automatic && !state.shutting_down;
        state.pending_automatic = false;
        pending
    }
}

impl Drop for PreparationLease<'_> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.active == Some(self.attempt) {
            state.active = None;
            // An abandoned caller cannot schedule its coalesced successor.
            state.pending_automatic = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn started<'a>(admission: PreparationAdmission<'a>) -> PreparationLease<'a> {
        match admission {
            PreparationAdmission::Started(lease) => lease,
            _ => panic!("preparação deveria ter sido admitida"),
        }
    }

    #[test]
    fn concurrent_automatic_requests_coalesce_without_starting_another_worker() {
        let state = Arc::new(ManagedPreparationCoordinator::new());
        let live = Path::new("/private/library");
        let first = started(state.begin(PreparationRequest::Automatic, live));
        let barrier = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_barrier = Arc::clone(&barrier);
        let worker = thread::spawn(move || {
            worker_barrier.wait();
            assert!(matches!(
                worker_state.begin(PreparationRequest::Automatic, live),
                PreparationAdmission::Coalesced
            ));
        });
        barrier.wait();
        worker.join().unwrap();
        assert!(matches!(
            state.begin(PreparationRequest::Explicit, live),
            PreparationAdmission::Busy
        ));
        assert!(first.finish());
        let second = started(state.begin(PreparationRequest::Automatic, live));
        assert!(!second.finish());
    }

    #[test]
    fn each_attempt_has_exclusive_staging_and_revisions_invalidate_old_results() {
        let state = ManagedPreparationCoordinator::new();
        let live = Path::new("/private/library");
        let first = started(state.begin(PreparationRequest::Explicit, live));
        let first_path = first.staging_path().to_path_buf();
        assert!(first.is_current());
        state.note_config_change();
        assert!(!first.is_current());
        assert!(!first.finish());

        let second = started(state.begin(PreparationRequest::Explicit, live));
        assert_ne!(second.staging_path(), first_path);
        assert!(second.is_current());
        state.note_generation_change();
        assert!(!second.is_current());
    }

    #[test]
    fn shutdown_invalidates_in_flight_work_and_rejects_late_admission() {
        let state = ManagedPreparationCoordinator::new();
        let live = Path::new("/private/library");
        let active = started(state.begin(PreparationRequest::Automatic, live));
        state.begin_shutdown_with(|| {});
        assert!(!active.is_current());
        assert!(!active.finish());
        assert!(matches!(
            state.begin(PreparationRequest::Explicit, live),
            PreparationAdmission::ShuttingDown
        ));
    }

    #[test]
    fn admission_racing_shutdown_cannot_leave_a_current_attempt() {
        let state = Arc::new(ManagedPreparationCoordinator::new());
        let barrier = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_barrier = Arc::clone(&barrier);
        let worker = thread::spawn(move || {
            worker_barrier.wait();
            let admission =
                worker_state.begin(PreparationRequest::Automatic, Path::new("/private/library"));
            worker_barrier.wait();
            match admission {
                PreparationAdmission::Started(lease) => assert!(!lease.is_current()),
                PreparationAdmission::ShuttingDown => {}
                _ => panic!("admissão inesperada"),
            }
        });
        barrier.wait();
        state.begin_shutdown_with(|| {});
        barrier.wait();
        worker.join().unwrap();
    }

    #[test]
    fn dropped_worker_releases_single_flight_slot() {
        let state = ManagedPreparationCoordinator::new();
        let live = Path::new("/private/library");
        let abandoned = started(state.begin(PreparationRequest::Automatic, live));
        drop(abandoned);
        let retry = started(state.begin(PreparationRequest::Explicit, live));
        assert!(retry.is_current());
    }
}
