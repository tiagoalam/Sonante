//! Session-only sleep timer. UI/window lifetimes never own the playback deadline.
use serde::Serialize;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
const EVENT: &str = "sleep-timer://changed";
const REPLACED: &str = "SLEEP_TIMER_REPLACED";
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Idle,
    Armed,
    Expired,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    revision: u64,
    phase: Phase,
    remaining_ms: u64,
}
struct Timer {
    revision: u64,
    generation: u64,
    phase: Phase,
    deadline: Option<Instant>,
}
impl Default for Timer {
    fn default() -> Self {
        Self {
            revision: 0,
            generation: 0,
            phase: Phase::Idle,
            deadline: None,
        }
    }
}
impl Timer {
    fn set(
        &mut self,
        minutes: Option<u32>,
        now: Instant,
    ) -> Result<Option<(u64, Duration)>, String> {
        if minutes.is_some_and(|m| ![15, 30, 60].contains(&m)) {
            return Err("SLEEP_TIMER_INVALID".into());
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("SLEEP_TIMER_UNAVAILABLE")?;
        self.revision = revision;
        self.generation = revision;
        self.phase = if minutes.is_some() {
            Phase::Armed
        } else {
            Phase::Idle
        };
        self.deadline = minutes.map(|m| now + Duration::from_secs(u64::from(m) * 60));
        Ok(self
            .deadline
            .map(|end| (self.generation, end.saturating_duration_since(now))))
    }
    fn due(&self, generation: u64, now: Instant) -> Result<(), String> {
        if self.generation == generation
            && self.phase == Phase::Armed
            && self.deadline.is_some_and(|end| now >= end)
        {
            Ok(())
        } else {
            Err(REPLACED.into())
        }
    }
    fn finish(&mut self, generation: u64, ok: bool) -> bool {
        if self.generation != generation || self.phase != Phase::Armed {
            return false;
        }
        self.phase = if ok { Phase::Expired } else { Phase::Failed };
        self.deadline = None;
        self.revision = self.revision.saturating_add(1);
        true
    }
    fn snapshot(&self, now: Instant) -> Snapshot {
        Snapshot {
            revision: self.revision,
            phase: self.phase,
            remaining_ms: self
                .deadline
                .map(|end| end.saturating_duration_since(now).as_millis() as u64)
                .unwrap_or(0),
        }
    }
}
#[derive(Default)]
pub struct State {
    inner: Mutex<Timer>,
    worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}
impl State {
    fn with_due<T>(
        &self,
        generation: u64,
        now: Instant,
        effect: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let timer = self.inner.lock().map_err(|_| "SLEEP_TIMER_UNAVAILABLE")?;
        timer.due(generation, now)?;
        let result = effect();
        drop(timer);
        result
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Ok(worker) = self.worker.get_mut() {
            if let Some(worker) = worker.take() {
                worker.abort();
            }
        }
    }
}
fn publish(app: &AppHandle, snapshot: Snapshot) {
    if app.emit(EVENT, snapshot).is_err() {
        eprintln!("[SleepTimer] notification_unavailable");
    }
}
#[tauri::command]
pub async fn get_sleep_timer(app: AppHandle) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let snapshot = state
            .inner
            .lock()
            .map_err(|_| "SLEEP_TIMER_UNAVAILABLE")?
            .snapshot(Instant::now());
        Ok(snapshot)
    })
    .await
    .map_err(|_| "SLEEP_TIMER_UNAVAILABLE".to_string())?
}
#[tauri::command]
pub async fn set_sleep_timer(app: AppHandle, minutes: Option<u32>) -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || set(app, minutes))
        .await
        .map_err(|_| "SLEEP_TIMER_UNAVAILABLE".to_string())?
}
fn set(app: AppHandle, minutes: Option<u32>) -> Result<Snapshot, String> {
    let transactions = app.state::<crate::ConfigTransactionState>();
    if minutes.is_some() && (transactions.shutdown_requested() || transactions.shutdown_started()) {
        return Err("SLEEP_TIMER_UNAVAILABLE".into());
    }
    let state = app.state::<State>();
    // Setters serialize only this timer. Sleeping never holds these locks.
    let mut worker = state.worker.lock().map_err(|_| "SLEEP_TIMER_UNAVAILABLE")?;
    let mut timer = state.inner.lock().map_err(|_| "SLEEP_TIMER_UNAVAILABLE")?;
    let next = timer.set(minutes, Instant::now())?;
    if let Some(previous) = worker.take() {
        previous.abort();
    }
    let snapshot = timer.snapshot(Instant::now());
    if let Some((generation, delay)) = next {
        let handle = app.clone();
        *worker = Some(tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            // Existing transport/admission is synchronous and may wait for AudioState.
            let failure_handle = handle.clone();
            if tauri::async_runtime::spawn_blocking(move || expire(&handle, generation))
                .await
                .is_err()
            {
                finish(
                    &failure_handle,
                    generation,
                    Err("SLEEP_TIMER_UNAVAILABLE".into()),
                );
            }
        }));
    }
    drop(timer);
    drop(worker);
    publish(&app, snapshot.clone());
    Ok(snapshot)
}
pub(crate) fn shutdown(app: &AppHandle) {
    if let Err(error) = set(app.clone(), None) {
        eprintln!(
            "[SleepTimer] shutdown_unavailable {}",
            crate::diagnostics::error_summary(&error)
        );
    }
}
fn expire(app: &AppHandle, generation: u64) {
    let state = app.state::<State>();
    let audio = app.state::<crate::AudioState>();
    let transactions = app.state::<crate::ConfigTransactionState>();
    // Recheck AFTER acquiring AudioState and BEFORE admitting a playback intent.
    let mut admission_guard = None;
    let token = crate::admit_or_validate_playback_checked(&audio.0, &transactions, None, |_| {
        let timer = state.inner.lock().map_err(|_| "SLEEP_TIMER_UNAVAILABLE")?;
        timer.due(generation, Instant::now())?;
        // Keep the timer lease through intent admission, not only its preliminary
        // check: an acknowledged cancellation must also fence a late admission.
        admission_guard = Some(timer);
        Ok(())
    });
    drop(admission_guard);
    let result = token.and_then(|token| {
        crate::finish_playback_intent(&audio.0, &transactions, &token, |engine| {
            // Audio -> timer is the only combined lock order. Cancellation/replacement
            // is acknowledged only after an already-started synchronous stop returns.
            state.with_due(generation, Instant::now(), || {
                audio.note_transport_intervention();
                engine.stop_for_sleep()
            })
        })
    });
    finish(app, generation, result);
}
fn finish(app: &AppHandle, generation: u64, result: Result<(), String>) {
    let state = app.state::<State>();
    if let Ok(mut timer) = state.inner.lock() {
        if timer.finish(generation, result.is_ok()) {
            if let Err(error) = result {
                eprintln!(
                    "[SleepTimer] stop_failed {}",
                    crate::diagnostics::error_summary(&error)
                );
            }
            let snapshot = timer.snapshot(Instant::now());
            drop(timer);
            publish(app, snapshot);
        }
    };
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_deadlines_cancel_replace_and_session_reset_without_late_effects() {
        let now = Instant::now();
        let mut timer = Timer::default();
        for invalid in [0, 1, 14, 16, 61, u32::MAX] {
            assert!(timer.set(Some(invalid), now).is_err());
        }
        assert_eq!(timer.revision, 0);
        for minutes in [15, 30, 60] {
            let (id, delay) = timer.set(Some(minutes), now).unwrap().unwrap();
            assert_eq!(delay.as_secs(), u64::from(minutes) * 60);
            assert!(timer
                .due(id, now + delay - Duration::from_millis(1))
                .is_err());
            assert!(timer.due(id, now + delay).is_ok());
            timer.set(None, now).unwrap();
            assert!(timer.due(id, now + delay).is_err());
            assert!(!timer.finish(id, true));
            assert_eq!(timer.phase, Phase::Idle);
        }
        let (old, _) = timer.set(Some(15), now).unwrap().unwrap();
        let (current, _) = timer.set(Some(60), now).unwrap().unwrap();
        assert!(timer.due(old, now + Duration::from_secs(3600)).is_err());
        assert!(!timer.finish(old, false));
        assert_eq!(timer.generation, current);
        assert_eq!(timer.phase, Phase::Armed);
        assert_eq!(Timer::default().phase, Phase::Idle);
    }
    #[test]
    fn canceled_replaced_or_early_deadlines_cannot_enter_the_transport_effect() {
        let state = State::default();
        let now = Instant::now();
        let (old, delay) = state
            .inner
            .lock()
            .unwrap()
            .set(Some(15), now)
            .unwrap()
            .unwrap();
        assert!(state
            .with_due(old, now, || -> Result<(), String> {
                panic!("early effect")
            })
            .is_err());
        state.inner.lock().unwrap().set(None, now).unwrap();
        assert!(state
            .with_due(old, now + delay, || -> Result<(), String> {
                panic!("canceled effect")
            })
            .is_err());
        let (current, _) = state
            .inner
            .lock()
            .unwrap()
            .set(Some(30), now)
            .unwrap()
            .unwrap();
        assert!(state
            .with_due(old, now + delay, || -> Result<(), String> {
                panic!("replaced effect")
            })
            .is_err());
        let error = state
            .with_due(current, now + Duration::from_secs(1800), || {
                Err::<(), _>("ACK".to_string())
            })
            .unwrap_err();
        assert_eq!(error, "ACK");
    }
    #[test]
    fn cancel_acknowledgement_cannot_precede_an_already_started_stop() {
        use std::sync::{mpsc, Arc};
        let state = Arc::new(State::default());
        let now = Instant::now();
        let (id, delay) = state
            .inner
            .lock()
            .unwrap()
            .set(Some(15), now)
            .unwrap()
            .unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = state.clone();
        let stop = std::thread::spawn(move || {
            worker
                .with_due(id, now + delay, || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
        });
        entered_rx.recv().unwrap();
        assert!(state.inner.try_lock().is_err());
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let cancel_state = state.clone();
        let cancel = std::thread::spawn(move || {
            cancel_state.inner.lock().unwrap().set(None, now).unwrap();
            cancel_tx.send(()).unwrap();
        });
        assert!(cancel_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        stop.join().unwrap();
        cancel.join().unwrap();
        cancel_rx.recv().unwrap();
        assert!(state
            .with_due(id, now + delay, || -> Result<(), String> {
                panic!("late effect after cancel acknowledgement")
            })
            .is_err());
    }
    #[test]
    fn completion_is_once_only_failure_is_not_success_and_no_later_retry() {
        for ok in [true, false] {
            let now = Instant::now();
            let mut timer = Timer::default();
            let (id, delay) = timer.set(Some(15), now).unwrap().unwrap();
            assert_eq!(
                timer.snapshot(now + Duration::from_secs(5)).remaining_ms,
                895000
            );
            let revision = timer.revision;
            assert!(timer.due(id, now + delay).is_ok());
            assert!(timer.finish(id, ok));
            assert_eq!(timer.phase, if ok { Phase::Expired } else { Phase::Failed });
            assert!(timer.revision > revision);
            assert_eq!(timer.snapshot(now).remaining_ms, 0);
            assert!(timer
                .due(id, now + delay + Duration::from_secs(99))
                .is_err());
            assert!(!timer.finish(id, true));
        }
    }
}
