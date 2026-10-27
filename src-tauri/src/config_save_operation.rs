//! Admission and ownership of Settings Save, including the interval between
//! accepting a background operation and starting its worker. No disk state lives here.
use crate::local_import_preview::{PreviewError, Scope};
use serde::Serialize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub(crate) const BUSY: &str = "CONFIG_SAVE_BUSY";
pub(crate) const INITIAL_CATALOG_INDEXING: &str = "FIRST_RUN_CATALOG_INDEXING";
pub(crate) const INTERRUPTED: &str = "LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Preparing,
    Indexing,
    Validating,
    Completed,
    Failed,
    Uncertain,
}

impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Uncertain)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Snapshot {
    pub id: String,
    pub phase: Phase,
    pub error: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub coverage: Option<crate::index_coverage::Report>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum SaveResponse {
    Completed { #[serde(skip_serializing_if = "Option::is_none")] warning: Option<SaveWarning> },
    Accepted { operation: Snapshot },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SaveWarning { PositionNotRestored }

impl SaveResponse {
    pub(crate) fn completed(outcome: crate::audio::DeviceSwitchRestoreOutcome) -> Self {
        Self::Completed { warning: match outcome {
            crate::audio::DeviceSwitchRestoreOutcome::Complete => None,
            crate::audio::DeviceSwitchRestoreOutcome::PositionNotRestored => Some(SaveWarning::PositionNotRestored),
        }}
    }
}

#[derive(Default, Clone, Copy)]
enum Purpose {
    #[default]
    Save,
    InitialCatalog,
}

#[derive(Default)]
struct Inner {
    busy: bool,
    shutdown: bool,
    next_id: u64,
    operation: Option<Snapshot>,
    purpose: Purpose,
    preview: Option<Scope>,
    preview_offsets: Vec<usize>,
    preview_source_start: usize,
    preview_busy: bool,
    preview_artwork_busy: bool,
    preview_not_before: Option<Instant>,
    preview_artwork_not_before: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BackgroundAdmission { Busy, Blocked }

pub(crate) struct Coordinator {
    inner: Mutex<Inner>,
    idle: Condvar,
}

#[derive(Debug)]
pub(crate) struct Transaction<'a> {
    coordinator: &'a Coordinator,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigSaveCoordinator")
    }
}

/// Owning, Send reservation. Shutdown waits even if the worker has not run yet.
pub(crate) struct Operation {
    coordinator: Arc<Coordinator>,
    id: String,
    finished: bool,
}

/// Read-only reservation, independent of the transaction and AudioState locks.
pub(crate) struct PreviewLease {
    coordinator: Arc<Coordinator>,
    id: String,
    pub(crate) scope: Scope,
    artwork: bool,
}

pub(crate) struct PreviewJob {
    coordinator: Arc<Coordinator>,
    id: String,
    job: u64,
}

impl Drop for PreviewJob {
    fn drop(&mut self) {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.operation.as_ref().is_some_and(|op| op.id == self.id) {
            if let Some(scope) = inner
                .preview
                .as_mut()
                .filter(|scope| scope.job == Some(self.job))
            {
                scope.job = None;
                inner.preview_offsets.clear();
            }
        }
    }
}

impl PreviewLease {
    pub(crate) fn source_start(&self) -> Result<usize, PreviewError> {
        let inner = self.coordinator.inner.lock().map_err(|_| PreviewError::Unavailable)?;
        if !self.valid(&inner) { return Err(PreviewError::Unavailable); }
        Ok(inner.preview_source_start)
    }
    fn valid(&self, inner: &Inner) -> bool {
        !inner.shutdown
            && inner
                .operation
                .as_ref()
                .is_some_and(|op| op.id == self.id && op.phase == Phase::Indexing)
            && inner.preview.as_ref() == Some(&self.scope)
    }

    pub(crate) fn ensure_active(&self) -> Result<(), PreviewError> {
        let inner = self
            .coordinator
            .inner
            .lock()
            .map_err(|_| PreviewError::Unavailable)?;
        if self.valid(&inner) {
            Ok(())
        } else {
            Err(PreviewError::Unavailable)
        }
    }

    pub(crate) fn offsets(&self) -> Result<Vec<usize>, PreviewError> {
        let inner = self
            .coordinator
            .inner
            .lock()
            .map_err(|_| PreviewError::Unavailable)?;
        if !self.valid(&inner) || inner.preview_offsets.len() != self.scope.sources.len() {
            return Err(PreviewError::Unavailable);
        }
        Ok(inner.preview_offsets.clone())
    }

    /// Confirm all pages atomically; a failed/obsolete sample moves no cursor.
    pub(crate) fn commit_offsets(
        &self,
        previous: &[usize],
        next: Vec<usize>,
    ) -> Result<(), PreviewError> {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .map_err(|_| PreviewError::Unavailable)?;
        if !self.valid(&inner)
            || inner.preview_offsets != previous
            || next.len() != self.scope.sources.len()
        {
            return Err(PreviewError::Unavailable);
        }
        inner.preview_offsets = next;
        if self.scope.initial {
            inner.preview_source_start = (inner.preview_source_start + 8) % self.scope.sources.len();
        }
        Ok(())
    }
}

impl Drop for PreviewLease {
    fn drop(&mut self) {
        let mut inner = self.coordinator.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.artwork { inner.preview_artwork_busy = false; } else { inner.preview_busy = false; }
    }
}

impl Coordinator {
    pub(crate) fn shutdown_requested(&self)->bool {
        self.inner.lock().map_or(true,|inner|inner.shutdown)
    }

    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            idle: Condvar::new(),
        }
    }

    fn admission_error(inner: &Inner) -> Option<String> {
        if inner.shutdown {
            Some("A aplicação está sendo encerrada; a transação não foi iniciada.".into())
        } else if inner
            .operation
            .as_ref()
            .is_some_and(|op| op.phase == Phase::Uncertain)
        {
            Some(crate::managed_save_intent::BLOCKED.into())
        } else {
            None
        }
    }

    /// Watcher admission is typed and never waits while holding its event
    /// reader. Busy retains a bounded batch; Uncertain/shutdown never retries.
    pub(crate) fn try_begin_background(&self) -> Result<Transaction<'_>, BackgroundAdmission> {
        let mut inner = self.inner.lock().map_err(|_| BackgroundAdmission::Blocked)?;
        if Self::admission_error(&inner).is_some() { return Err(BackgroundAdmission::Blocked); }
        if inner.busy { return Err(BackgroundAdmission::Busy); }
        inner.busy = true;
        Ok(Transaction { coordinator: self })
    }

    pub(crate) fn begin(&self) -> Result<Transaction<'_>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Controle de Save indisponível.")?;
        loop {
            if let Some(error) = Self::admission_error(&inner) {
                return Err(error);
            }
            if !inner.busy {
                break;
            }
            if inner
                .operation
                .as_ref()
                .is_some_and(|op| !op.phase.terminal())
            {
                return Err(BUSY.into());
            }
            inner = self
                .idle
                .wait(inner)
                .map_err(|_| "Controle de Save indisponível.")?;
        }
        inner.busy = true;
        Ok(Transaction { coordinator: self })
    }

    pub(crate) fn reserve(self: &Arc<Self>) -> Result<(Snapshot, Operation), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Controle de Save indisponível.")?;
        if let Some(error) = Self::admission_error(&inner) {
            return Err(error);
        }
        if inner.busy {
            return Err(BUSY.into());
        }
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .ok_or("Identidade de Save esgotada.")?;
        let snapshot = Snapshot {
            id: inner.next_id.to_string(),
            phase: Phase::Preparing,
            error: None,
            coverage: None,
        };
        inner.operation = Some(snapshot.clone());
        inner.purpose = Purpose::Save;
        inner.preview = None;
        inner.preview_offsets.clear();
        inner.busy = true;
        Ok((
            snapshot.clone(),
            Operation {
                coordinator: Arc::clone(self),
                id: snapshot.id,
                finished: false,
            },
        ))
    }

    // Explicit startup recovery may resume its own uncertain operation. The
    // admission capability is issued only for the current analyzed plan.
    pub(crate) fn reserve_recovery(self: &Arc<Self>, _admission: crate::library_recovery::RecoveryAdmission) -> Result<(Snapshot, Operation), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Controle de Save indisponível.")?;
        if inner.shutdown { return Err(INTERRUPTED.into()); }
        if inner.busy {
            return Err(BUSY.into());
        }
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .ok_or("Identidade de Save esgotada.")?;
        let snapshot = Snapshot {
            id: inner.next_id.to_string(),
            phase: Phase::Preparing,
            error: None,
            coverage: None,
        };
        inner.operation = Some(snapshot.clone());
        inner.preview = None;
        inner.preview_offsets.clear();
        inner.busy = true;
        Ok((
            snapshot.clone(),
            Operation {
                coordinator: Arc::clone(self),
                id: snapshot.id,
                finished: false,
            },
        ))
    }

    pub(crate) fn snapshot(&self) -> Result<Option<Snapshot>, String> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| "Controle de Save indisponível.")?
            .operation
            .clone())
    }

    pub(crate) fn reserve_preview(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<PreviewLease, PreviewError> {
        let mut inner = self.inner.lock().map_err(|_| PreviewError::Unavailable)?;
        if inner.shutdown
            || !inner
                .operation
                .as_ref()
                .is_some_and(|op| op.id == id && op.phase == Phase::Indexing)
        {
            return Err(PreviewError::Unavailable);
        }
        let scope = inner
            .preview
            .clone()
            .filter(|scope| scope.job.is_some())
            .ok_or(PreviewError::Unavailable)?;
        if inner.preview_busy
            || inner
                .preview_not_before
                .is_some_and(|at| Instant::now() < at)
        {
            return Err(PreviewError::Busy);
        }
        inner.preview_busy = true;
        inner.preview_not_before = Some(Instant::now() + Duration::from_millis(500));
        Ok(PreviewLease {
            coordinator: Arc::clone(self),
            id: id.to_string(),
            scope,
            artwork: false,
        })
    }
    pub(crate) fn reserve_preview_artwork(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<PreviewLease, PreviewError> {
        let mut inner = self.inner.lock().map_err(|_| PreviewError::Unavailable)?;
        if inner.shutdown
            || !inner
                .operation
                .as_ref()
                .is_some_and(|op| op.id == id && op.phase == Phase::Indexing)
        {
            return Err(PreviewError::Unavailable);
        }
        let scope = inner
            .preview
            .clone()
            .filter(|scope| scope.job.is_some())
            .ok_or(PreviewError::Unavailable)?;
        if inner.preview_artwork_busy
            || inner
                .preview_artwork_not_before
                .is_some_and(|at| Instant::now() < at)
        {
            return Err(PreviewError::Busy);
        }
        inner.preview_artwork_busy = true;
        inner.preview_artwork_not_before = Some(Instant::now() + Duration::from_millis(500));
        Ok(PreviewLease {
            coordinator: Arc::clone(self),
            id: id.to_string(),
            scope,
            artwork: true,
        })
    }

    pub(crate) fn active(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .operation
            .as_ref()
            .is_some_and(|op| !op.phase.terminal())
    }

    pub(crate) fn reject_active(&self) -> Result<(), String> {
        self.reject_active_with_feedback(false)
    }

    /// Same refusal, with actionable playback feedback only after saved
    /// first-run preferences have handed ownership to the catalog worker.
    pub(crate) fn initial_catalog_id(&self) -> Result<Option<String>, String> {
        let inner = self.inner.lock().map_err(|_| "Playback unavailable")?;
        if let Some(error) = Self::admission_error(&inner) { return Err(error); }
        Ok(inner.operation.as_ref().filter(|op| !op.phase.terminal()
            && matches!(inner.purpose, Purpose::InitialCatalog)).map(|op| op.id.clone()))
    }

    pub(crate) fn reject_playback(&self) -> Result<(), String> {
        self.reject_active_with_feedback(true)
    }

    fn reject_active_with_feedback(&self, playback: bool) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Controle de Save indisponível.")?;
        if let Some(error) = Self::admission_error(&inner) {
            return Err(error);
        }
        if inner
            .operation
            .as_ref()
            .is_some_and(|op| !op.phase.terminal())
        {
            Err(if playback && matches!(inner.purpose, Purpose::InitialCatalog) {
                INITIAL_CATALOG_INDEXING
            } else { BUSY }.into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn request_shutdown(&self, mark: impl FnOnce()) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.shutdown = true;
        inner.preview = None;
        inner.preview_offsets.clear();
        mark();
        self.idle.notify_all();
    }

    pub(crate) fn wait_for_idle(&self) -> Transaction<'_> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while inner.busy {
            inner = self
                .idle
                .wait(inner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        inner.busy = true;
        Transaction { coordinator: self }
    }
}

impl Transaction<'_> {
    pub(crate) fn belongs_to(&self, coordinator: &Coordinator) -> bool {
        std::ptr::eq(self.coordinator, coordinator)
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.busy = false;
        self.coordinator.idle.notify_all();
    }
}

impl Operation {
    pub(crate) fn initial_catalog_background(&self) {
        let mut inner = self.coordinator.inner.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.operation.as_ref().is_some_and(|op| op.id == self.id && !op.phase.terminal()) {
            inner.purpose = Purpose::InitialCatalog;
        }
    }

    /// Only the owned initial-generation worker can enable this scope. Each
    /// read still has at most eight source windows and 128 tracks total.
    pub(crate) fn enable_initial_preview(&self, socket: &str, sources: &[String]) -> Result<(), PreviewError> {
        let scope = Scope::initial(socket, sources)?;
        let mut inner = self.coordinator.inner.lock().map_err(|_| PreviewError::Unavailable)?;
        if inner.shutdown || !inner.operation.as_ref().is_some_and(|op| op.id == self.id && !op.phase.terminal()) {
            return Err(PreviewError::Unavailable);
        }
        inner.preview = Some(scope);
        inner.preview_offsets.clear();
        inner.preview_source_start = 0;
        Ok(())
    }
    /// Called only after a proven targeted addition has been published.
    pub(crate) fn enable_addition_preview(
        &self,
        socket: &str,
        sources: &[String],
    ) -> Result<(), PreviewError> {
        let scope = Scope::new(socket, sources)?;
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .map_err(|_| PreviewError::Unavailable)?;
        if inner.shutdown
            || !inner
                .operation
                .as_ref()
                .is_some_and(|op| op.id == self.id && !op.phase.terminal())
        {
            return Err(PreviewError::Unavailable);
        }
        inner.preview = Some(scope);
        inner.preview_offsets.clear();
        inner.preview_source_start = 0;
        Ok(())
    }

    pub(crate) fn enable_preview_artwork(
        &self,
        artwork: Option<crate::local_import_artwork::ArtworkScope>,
    ) {
        let Ok(mut inner) = self.coordinator.inner.lock() else {
            return;
        };
        if !inner.shutdown
            && inner
                .operation
                .as_ref()
                .is_some_and(|op| op.id == self.id && !op.phase.terminal())
        {
            if let Some(scope) = inner.preview.as_mut() {
                scope.artwork = artwork.filter(|artwork| artwork.matches_sources(&scope.sources));
            }
        }
    }

    pub(crate) fn preview_job(&self, job: u64) -> PreviewJob {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.shutdown
            && inner
                .operation
                .as_ref()
                .is_some_and(|op| op.id == self.id && op.phase == Phase::Indexing)
        {
            if let Some(scope) = inner.preview.as_mut() {
                scope.job = Some(job);
                inner.preview_offsets = vec![0; scope.sources.len()];
            }
        }
        PreviewJob {
            coordinator: Arc::clone(&self.coordinator),
            id: self.id.clone(),
            job,
        }
    }

    pub(crate) fn coverage(&self, report: crate::index_coverage::Report) {
        if let Ok(mut inner) = self.coordinator.inner.lock() {
            if let Some(op) = inner.operation.as_mut().filter(|op| op.id == self.id && !op.phase.terminal()) { op.coverage = Some(report); }
        }
    }

    pub(crate) fn phase(&self, phase: Phase) {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(op) = inner.operation.as_mut().filter(|op| op.id == self.id) {
            if !op.phase.terminal() && !phase.terminal() {
                op.phase = phase;
                if phase == Phase::Validating {
                    inner.preview = None;
                    inner.preview_offsets.clear();
                }
            }
        }
    }

    pub(crate) fn shutdown_requested(&self) -> bool {
        self.coordinator
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown
    }

    pub(crate) fn finish(mut self, result: Result<(), String>, uncertain: bool) {
        self.finish_inner(result, uncertain);
        self.finished = true;
    }

    fn finish_inner(&self, result: Result<(), String>, uncertain: bool) {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(op) = inner.operation.as_mut().filter(|op| op.id == self.id) {
            op.phase = if uncertain {
                Phase::Uncertain
            } else if result.is_ok() {
                Phase::Completed
            } else {
                Phase::Failed
            };
            op.error = result.err();
            inner.preview = None;
            inner.preview_offsets.clear();
            inner.busy = false;
            self.coordinator.idle.notify_all();
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if !self.finished {
            self.finish_inner(
                Err("A tarefa de Save foi interrompida; conclusão não confirmada.".into()),
                true,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn initial_catalog_feedback_preserves_all_admission_barriers() {
        let state = Arc::new(Coordinator::new());
        let (_, operation) = state.reserve().unwrap();
        assert_eq!(state.reject_playback().unwrap_err(), BUSY);
        operation.initial_catalog_background();
        for phase in [Phase::Preparing, Phase::Indexing, Phase::Validating] {
            operation.phase(phase);
            assert_eq!(state.reject_playback().unwrap_err(), INITIAL_CATALOG_INDEXING);
            assert_eq!(state.reject_active().unwrap_err(), BUSY);
            assert!(state.try_begin_background().is_err());
            assert!(state.reserve().is_err());
        }
        operation.finish(Ok(()), false);
        assert!(state.reject_playback().is_ok());
        let (_, operation) = state.reserve().unwrap();
        assert_eq!(state.reject_playback().unwrap_err(), BUSY);
        operation.initial_catalog_background();
        operation.finish(Err("fixture".into()), true);
        assert_eq!(state.reject_playback().unwrap_err(), crate::managed_save_intent::BLOCKED);
    }

    #[test]
    fn preview_offsets_are_discarded_at_terminal_validation_and_shutdown_boundaries() {
        for ending in 0..5 {
            let state = Arc::new(Coordinator::new());
            let (accepted, op) = state.reserve().unwrap();
            op.enable_addition_preview("unused", &["Nova".into()])
                .unwrap();
            op.phase(Phase::Indexing);
            let _job = op.preview_job(7);
            let lease = state.reserve_preview(&accepted.id).unwrap();
            lease.commit_offsets(&[0], vec![128]).unwrap();
            match ending {
                0 => op.finish(Ok(()), false),
                1 => op.finish(Err("failed".into()), false),
                2 => op.finish(Err("uncertain".into()), true),
                3 => op.phase(Phase::Validating),
                _ => state.request_shutdown(|| {}),
            }
            assert!(state.inner.lock().unwrap().preview_offsets.is_empty());
            assert_eq!(
                lease.commit_offsets(&[128], vec![256]),
                Err(PreviewError::Unavailable)
            );
            if ending < 2 {
                let (_, next) = state.reserve().unwrap();
                next.enable_addition_preview("unused", &["Nova".into(), "Outra".into()])
                    .unwrap();
                next.phase(Phase::Indexing);
                let _next_job = next.preview_job(8);
                assert_eq!(state.inner.lock().unwrap().preview_offsets, [0, 0]);
                assert_eq!(
                    lease.commit_offsets(&[128], vec![256]),
                    Err(PreviewError::Unavailable)
                );
                assert_eq!(state.inner.lock().unwrap().preview_offsets, [0, 0]);
            }
        }
    }

    #[test]
    fn acceptance_owns_transaction_before_worker_starts_and_keeps_pending_phases() {
        let state = Arc::new(Coordinator::new());
        let (accepted, worker) = state.reserve().unwrap();
        assert_eq!(accepted.phase, Phase::Preparing);
        assert_eq!(state.reserve().err().unwrap(), BUSY);
        assert_eq!(state.begin().unwrap_err(), BUSY);
        for phase in [Phase::Indexing, Phase::Validating] {
            worker.phase(phase);
            assert_eq!(state.snapshot().unwrap().unwrap().phase, phase);
            assert!(state.active());
        }
        worker.finish(Ok(()), false);
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Completed);
        assert!(!state.active());
        assert!(state.begin().is_ok());
    }

    #[test]
    fn ordinary_transaction_cannot_race_background_admission() {
        let state = Arc::new(Coordinator::new());
        let guard = state.begin().unwrap();
        assert_eq!(state.reserve().err().unwrap(), BUSY);
        drop(guard);
        let (_, worker) = state.reserve().unwrap();
        worker.finish(Err("falha recuperada".into()), false);
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Failed);
        assert!(state.begin().is_ok());
    }

    #[test]
    fn shutdown_waits_for_accepted_not_yet_started_worker_and_rejects_saves() {
        let state = Arc::new(Coordinator::new());
        let (_, worker) = state.reserve().unwrap();
        state.request_shutdown(|| {});
        assert!(worker.shutdown_requested());
        assert!(state.reserve().is_err());
        let (tx, rx) = mpsc::channel();
        let shutdown_state = Arc::clone(&state);
        let shutdown = thread::spawn(move || {
            let _guard = shutdown_state.wait_for_idle();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        worker.finish(Err(INTERRUPTED.into()), true);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        shutdown.join().unwrap();
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Uncertain);
    }

    #[test]
    fn abandoned_or_panicked_worker_is_uncertain_and_cannot_authorize_next_save() {
        let state = Arc::new(Coordinator::new());
        let (_, worker) = state.reserve().unwrap();
        let _ = std::panic::catch_unwind(|| {
            let _worker = worker;
            panic!("fixture");
        });
        assert_eq!(state.snapshot().unwrap().unwrap().phase, Phase::Uncertain);
        assert!(!state.active());
        assert!(state.reserve().is_err());
        assert!(state.begin().is_err());
    }

    #[test]
    fn concurrent_acceptance_has_one_owner_and_shutdown_orders_against_admission() {
        use std::sync::Barrier;
        let state = Arc::new(Coordinator::new());
        let barrier = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let state = Arc::clone(&state);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    state.reserve()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        drop(results);

        let state = Arc::new(Coordinator::new());
        let barrier = Arc::new(Barrier::new(2));
        let reserve_state = Arc::clone(&state);
        let reserve_barrier = Arc::clone(&barrier);
        let admission = thread::spawn(move || {
            reserve_barrier.wait();
            reserve_state.reserve()
        });
        barrier.wait();
        state.request_shutdown(|| {});
        if let Ok((_, operation)) = admission.join().unwrap() {
            assert!(operation.shutdown_requested());
            operation.finish(Err(INTERRUPTED.into()), false);
        }
        assert!(state.reserve().is_err());
    }
}
