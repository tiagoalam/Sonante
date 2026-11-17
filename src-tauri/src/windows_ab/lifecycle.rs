//! Logical cancellation never releases the physical loader/decoder permit.
use crate::windows_shared::ab_comparison as ab;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
fn selected() -> &'static Mutex<Option<(String, ab::Check)>> {
    static S: OnceLock<Mutex<Option<(String, ab::Check)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}
pub(crate) fn begin(policy: ab::Check) -> Result<String, String> {
    if !policy() {
        return Err("AB_CHANGED".into());
    }
    let mut selected = selected().lock().map_err(|_| "AB_UNAVAILABLE")?;
    let token = ab::begin()?;
    *selected = Some((token.clone(), policy));
    Ok(token)
}
pub(crate) fn policy(token: &str) -> Result<ab::Check, String> {
    let policy = selected()
        .lock()
        .map_err(|_| "AB_UNAVAILABLE")?
        .as_ref()
        .filter(|(t, _)| t == token)
        .map(|(_, p)| p.clone())
        .ok_or("AB_CHANGED")?;
    let weak = Arc::downgrade(&ab::job(token)?);
    let check: ab::Check =
        Arc::new(move || policy() && weak.upgrade().is_some_and(|j| j.current()));
    if !check() {
        return Err("AB_CHANGED".into());
    }
    Ok(check)
}
pub(crate) fn cancel(token: &str) {
    if let Ok(mut selected) = selected().lock() {
        if selected.as_ref().is_some_and(|(t, _)| t == token) {
            ab::cancel(token);
            selected.take();
        }
    }
}
pub(crate) fn shutdown() {
    if let Ok(mut selected) = selected().lock() {
        if let Some((token, _)) = selected.take() {
            ab::cancel(&token);
        }
    }
}
pub(crate) fn slots() -> &'static Arc<tokio::sync::Semaphore> {
    static S: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    S.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
}
pub(crate) struct Pending {
    token: String,
    live: Arc<AtomicBool>,
    accepted: bool,
}
impl Pending {
    pub(crate) fn new(token: &str) -> Self {
        Self {
            token: token.into(),
            live: Arc::new(AtomicBool::new(true)),
            accepted: false,
        }
    }
    pub(crate) fn bind(&self, policy: ab::Check) -> ab::Check {
        let live = self.live.clone();
        Arc::new(move || live.load(Ordering::Acquire) && policy())
    }
    pub(crate) fn accept(&mut self) {
        self.accepted = true;
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.accepted {
            self.live.store(false, Ordering::Release);
            cancel(&self.token);
        }
    }
}
/// Both IPC load and decode use this exact owner. Dropping the awaiter cancels
/// logical publication, while the actual blocking closure still owns capacity.
pub(crate) async fn run<T, E, F>(
    token: String,
    budget: std::time::Duration,
    work: F,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<&'static str> + Send + 'static,
    F: FnOnce(ab::Check) -> Result<T, E> + Send + 'static,
{
    let permit = slots()
        .clone()
        .try_acquire_owned()
        .map_err(|_| E::from("AB_BUSY"))?;
    let mut pending = Pending::new(&token);
    let check = pending.bind(policy(&token).map_err(|_| E::from("AB_CHANGED"))?);
    let worker = tokio::task::spawn_blocking(move || {
        let _physical = permit;
        if !check() {
            return Err(E::from("AB_CHANGED"));
        }
        let result = work(check.clone())?;
        if !check() {
            return Err(E::from("AB_CHANGED"));
        }
        Ok(result)
    });
    let result = tokio::time::timeout(budget, worker)
        .await
        .map_err(|_| E::from("AB_UNAVAILABLE"))?
        .map_err(|_| E::from("AB_UNAVAILABLE"))??;
    pending.accept();
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abort_invalidates_input_without_cancelling_a_new_job() {
        let _serial = crate::windows_shared::AB_TEST_SERIAL.lock().unwrap();
        let token = begin(Arc::new(|| true)).unwrap();
        let pending = Pending::new(&token);
        let old = pending.bind(policy(&token).unwrap());
        assert!(old());
        let next = begin(Arc::new(|| true)).unwrap();
        drop(pending);
        assert!(!old());
        assert!(policy(&next).unwrap()());
        cancel(&next);
    }
    #[test]
    fn accepted_scope_does_not_revive_after_disable() {
        let _serial = crate::windows_shared::AB_TEST_SERIAL.lock().unwrap();
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let g = generation.clone();
        let token = begin(Arc::new(move || g.load(Ordering::Acquire) == 0)).unwrap();
        let mut pending = Pending::new(&token);
        let check = pending.bind(policy(&token).unwrap());
        pending.accept();
        drop(pending);
        assert!(check());
        generation.store(2, Ordering::Release);
        assert!(!check());
        cancel(&token);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_run_caller_timeout_keeps_physical_capacity_and_rejects_late_result() {
        let _serial = crate::windows_shared::AB_TEST_SERIAL.lock().unwrap();
        let token = begin(Arc::new(|| true)).unwrap();
        let (release, waiting) = std::sync::mpsc::channel();
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let old = token.clone();
        let caller = tokio::spawn(async move {
            run(
                old,
                std::time::Duration::from_millis(50),
                move |check| -> Result<(), String> {
                    signal.notify_one();
                    waiting.recv().unwrap();
                    assert!(!check());
                    Ok(())
                },
            )
            .await
        });
        entered.notified().await;
        assert_eq!(
            caller.await.unwrap().err().as_deref(),
            Some("AB_UNAVAILABLE")
        );
        assert!(ab::job(&token).is_err());
        assert_eq!(slots().available_permits(), 0);
        let next = begin(Arc::new(|| true)).unwrap();
        assert_eq!(
            run(next.clone(), std::time::Duration::from_secs(1), |_| Ok::<
                _,
                String,
            >(
                ()
            ))
            .await
            .err()
            .as_deref(),
            Some("AB_BUSY")
        );
        assert!(policy(&next).unwrap()());
        release.send(()).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while slots().available_permits() == 0 {
            assert!(std::time::Instant::now() < end);
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        run(next.clone(), std::time::Duration::from_secs(1), |check| {
            assert!(check());
            Ok::<_, String>(())
        })
        .await
        .unwrap();
        cancel(&next);
    }
}
