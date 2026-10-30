//! Read/control fencing for one explicit check of an already confirmed catalog.
//! The Settings transaction still excludes every competing catalog/config writer.
//! No guard here authorizes physical paths, source mutations or first-run reads.
use crate::config_save_operation::{Coordinator, Transaction};
use std::{
    cell::Cell,
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        RwLock, RwLockReadGuard, RwLockWriteGuard,
    },
};

pub(crate) const BLOCKED: &str = "LOCAL_CHECK_PLAYBACK_UNCONFIRMED";
#[derive(Default)]
enum State {
    #[default]
    Idle,
    Preparing,
    Checking {
        files: HashSet<String>,
        folders: HashSet<String>,
    },
    Blocked,
}
#[derive(Default)]
pub(crate) struct Gate {
    state: RwLock<State>,
    phase: AtomicU8,
    artwork_ready: AtomicBool,
}
pub(crate) struct Session<'a> {
    gate: &'a Gate,
    published: Cell<bool>,
    confirmed: Cell<bool>,
}
pub(crate) struct Control<'a> {
    gate: &'a Gate,
    _state: RwLockReadGuard<'a, State>,
}
pub(crate) struct Publication<'a> {
    _state: RwLockWriteGuard<'a, State>,
}
impl Gate {
    #[cfg(test)]
    pub(crate) fn begin<'a>(
        &'a self,
        owner: &'a Transaction<'_>,
        coordinator: &Coordinator,
        records: &HashSet<String>,
    ) -> Result<Session<'a>, String> {
        let session = self.begin_preparing(owner, coordinator)?;
        session.bind_catalog(records)?;
        Ok(session)
    }
    pub(crate) fn begin_preparing<'a>(
        &'a self,
        owner: &'a Transaction<'_>,
        coordinator: &Coordinator,
    ) -> Result<Session<'a>, String> {
        if !std::ptr::eq(self, coordinator.manual_check_playback.as_ref())
            || !owner.belongs_to(coordinator)
            || coordinator.shutdown_requested()
        {
            return Err(BLOCKED.into());
        }
        let mut state = self.state.write().map_err(|_| BLOCKED)?;
        if !matches!(*state, State::Idle) {
            return Err(BLOCKED.into());
        }
        *state = State::Preparing;
        self.phase.store(1, Ordering::SeqCst);
        Ok(Session {
            gate: self,
            published: Cell::new(false),
            confirmed: Cell::new(false),
        })
    }
    pub(crate) fn artwork_ready(&self) -> bool {
        self.active() && self.artwork_ready.load(Ordering::SeqCst)
    }
    pub(crate) fn active(&self) -> bool {
        self.phase.load(Ordering::SeqCst) == 1
    }
    pub(crate) fn admit(&self) -> Result<(), String> {
        if self.phase.load(Ordering::SeqCst) == 2 {
            Err(BLOCKED.into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn control(&self) -> Result<Control<'_>, String> {
        self.admit()?;
        let state = self.state.read().map_err(|_| BLOCKED)?;
        if matches!(*state, State::Blocked) {
            return Err(BLOCKED.into());
        }
        self.admit()?;
        Ok(Control {
            gate: self,
            _state: state,
        })
    }
    pub(crate) fn album<T>(
        &self,
        read: impl FnOnce(Option<&HashSet<String>>) -> Result<T, String>,
    ) -> Result<T, String> {
        // Retain the lease for ordinary reads too: a newly starting Check must
        // drain an already-running count/window read before freezing its catalog.
        let lease = self.control()?;
        match &*lease._state {
            State::Checking { files, .. } => read(Some(files)),
            _ => read(None),
        }
    }
}
impl Control<'_> {
    pub(crate) fn cover_ready(&self) -> bool {
        self.current() && matches!(*self._state, State::Checking { .. })
    }
    pub(crate) fn admits_cover(&self, coordinator: &Coordinator, uri: &str) -> bool {
        if !std::ptr::eq(self.gate, coordinator.manual_check_playback.as_ref())
            || coordinator.shutdown_requested()
            || !self.current()
        {
            return false;
        }
        let State::Checking { files, folders } = &*self._state else {
            return false;
        };
        // Exact component boundary. Build directory keys once per Check, rather
        // than scanning the entire track set at every artwork FD validation.
        files.contains(uri) || folders.contains(uri)
    }

    pub(crate) fn is_check(&self) -> bool {
        matches!(*self._state, State::Preparing | State::Checking { .. })
    }
    pub(crate) fn current(&self) -> bool {
        self.gate.admit().is_ok()
    }
}
impl Session<'_> {
    pub(crate) fn bind_catalog(&self, records: &HashSet<String>) -> Result<(), String> {
        let mut state = self.gate.state.write().map_err(|_| BLOCKED)?;
        self.gate.admit()?;
        if !matches!(*state, State::Preparing) {
            return Err(BLOCKED.into());
        }
        let files: HashSet<String> = records
            .iter()
            .filter_map(|row| row.strip_prefix("file: ").map(str::to_owned))
            .collect();
        let folders = files
            .iter()
            .flat_map(|file| {
                file.match_indices('/')
                    .map(move |(offset, _)| file[..offset].to_owned())
            })
            .collect();
        *state = State::Checking { files, folders };
        self.gate.artwork_ready.store(true, Ordering::SeqCst);
        Ok(())
    }
    // Called by the publication hook AFTER expensive tree validation. Readers
    // and controls drain only for the actual exchange, not for the whole scan.
    pub(crate) fn publication(&self) -> Result<Publication<'_>, String> {
        let state = self.gate.state.write().map_err(|_| BLOCKED)?;
        self.gate.admit()?;
        self.published.set(true);
        Ok(Publication { _state: state })
    }
    pub(crate) fn revoke(&self) {
        // Revoke first; do not hold AudioState while draining existing readers.
        self.gate.artwork_ready.store(false, Ordering::SeqCst);
        self.gate.phase.store(2, Ordering::SeqCst);
        let mut state = self
            .gate
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = State::Blocked;
    }
    pub(crate) fn confirm(&self) {
        self.confirmed.set(true);
    }
}
impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.gate.artwork_ready.store(false, Ordering::SeqCst);
        let safe = !self.published.get() || self.confirmed.get();
        let mut state = self
            .gate
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = if safe { State::Idle } else { State::Blocked };
        self.gate
            .phase
            .store(if safe { 0 } else { 2 }, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::Duration,
    };
    #[test]
    fn check_preserves_writer_exclusion_and_only_exposes_confirmed_file_records() {
        let coordinator = Arc::new(Coordinator::new());
        let owner = coordinator.begin().unwrap();
        let gate = coordinator.manual_check_playback.clone();
        let records = ["file: Music/one.flac".into(), "directory: Music".into()].into();
        let session = gate.begin(&owner, &coordinator, &records).unwrap();
        assert!(coordinator.try_begin_background().is_err());
        assert!(coordinator.reserve().is_err());
        assert_eq!(
            gate.album(|files| Ok(files.unwrap().clone())).unwrap(),
            ["Music/one.flac".into()].into()
        );
        assert!(gate.control().is_ok());
        assert!(gate.begin(&owner, &coordinator, &records).is_err());
        assert!(Gate::default()
            .begin(&owner, &coordinator, &records)
            .is_err());
        let foreign = Coordinator::new();
        assert!(Gate::default().begin(&owner, &foreign, &records).is_err());
        drop(session);
        assert!(!gate.active());
        assert!(gate.admit().is_ok());
        assert!(coordinator.try_begin_background().is_err());
        drop(owner);
        assert!(coordinator.try_begin_background().is_ok());
    }
    #[test]
    fn controls_run_during_preparation_and_update_but_exchange_waits_for_existing_control() {
        let coordinator = Coordinator::new();
        let gate = coordinator.manual_check_playback.clone();
        let owner = coordinator.begin().unwrap();
        let session = gate
            .begin(&owner, &coordinator, &Default::default())
            .unwrap();
        let control = gate.control().unwrap();
        let session = thread::scope(|scope| {
            let (started_tx, started_rx) = mpsc::channel();
            let (published_tx, published_rx) = mpsc::channel();
            let worker = scope.spawn(move || {
                started_tx.send(()).unwrap();
                let publication = session.publication().unwrap();
                published_tx.send(()).unwrap();
                drop(publication);
                session
            });
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(published_rx
                .recv_timeout(Duration::from_millis(20))
                .is_err());
            drop(control);
            published_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            worker.join().unwrap()
        });
        assert!(gate.control().is_ok()); // MPD update/audit retain normal controls.
        session.confirm();
        drop(session);
        assert!(gate.admit().is_ok());
    }
    #[test]
    fn revoke_closes_new_admission_before_draining_and_uncertain_publication_stays_blocked() {
        let coordinator = Coordinator::new();
        let gate = coordinator.manual_check_playback.clone();
        let owner = coordinator.begin().unwrap();
        let session = gate
            .begin(&owner, &coordinator, &Default::default())
            .unwrap();
        drop(session.publication().unwrap());
        let control = gate.control().unwrap();
        let session = thread::scope(|scope| {
            let (done_tx, done_rx) = mpsc::channel();
            let worker = scope.spawn(move || {
                session.revoke();
                done_tx.send(()).unwrap();
                session
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while gate.active() {
                assert!(std::time::Instant::now() < deadline);
                thread::yield_now();
            }
            assert_eq!(gate.admit().unwrap_err(), BLOCKED);
            assert!(done_rx.try_recv().is_err());
            drop(control);
            done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            worker.join().unwrap()
        });
        assert!(gate.album(|_| Ok(())).is_err());
        drop(session);
        assert!(gate.admit().is_err());
        assert!(gate
            .begin(&owner, &coordinator, &Default::default())
            .is_err());
    }
    #[test]
    fn clean_prepublication_failure_and_verified_recovery_release_only_their_own_session() {
        for published in [false, true] {
            let coordinator = Coordinator::new();
            let gate = coordinator.manual_check_playback.clone();
            let owner = coordinator.begin().unwrap();
            let session = gate
                .begin(&owner, &coordinator, &Default::default())
                .unwrap();
            if published {
                drop(session.publication().unwrap());
                session.revoke();
                session.confirm();
            }
            drop(session);
            assert!(gate.admit().is_ok());
            assert!(!gate.active());
        }
        let coordinator = Coordinator::new();
        let gate = coordinator.manual_check_playback.clone();
        let owner = coordinator.begin().unwrap();
        coordinator.request_shutdown(|| {});
        assert!(gate
            .begin(&owner, &coordinator, &Default::default())
            .is_err());
    }
    #[test]
    fn covers_require_bound_records_and_cannot_extend_the_check_or_escape_a_component() {
        let coordinator = Coordinator::new();
        let transaction = coordinator.begin().unwrap();
        let gate = coordinator.manual_check_playback.clone();
        let session = gate.begin_preparing(&transaction, &coordinator).unwrap();
        assert!(!gate.artwork_ready());
        {
            let control = gate.control().unwrap();
            assert!(!control.cover_ready());
            assert!(!control.admits_cover(&coordinator, "Music/Album"));
        }
        session
            .bind_catalog(
                &[
                    "file: Music/Album/one.flac".into(),
                    "file: Music/Disc.cue/track0001".into(),
                ]
                .into(),
            )
            .unwrap();
        assert!(gate.artwork_ready());
        {
            let control = gate.control().unwrap();
            assert!(control.cover_ready());
            for path in ["Music/Album", "Music/Album/one.flac", "Music/Disc.cue"] {
                assert!(control.admits_cover(&coordinator, path));
            }
            for path in [
                "Music/Al",
                "Music/Album2",
                "Music/Album/new.flac",
                "Music/Album/../Other",
            ] {
                assert!(!control.admits_cover(&coordinator, path));
            }
        }
        session.revoke();
        assert!(!gate.artwork_ready());
        assert!(gate.control().is_err());
    }
}
