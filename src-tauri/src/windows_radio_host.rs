//! Bounded Windows host for the original radio directory/store policy.
//! No file/network work at construction; I/O begins only for original UI calls.
use crate::windows_radio::{self as radio, Filters, Page, Search, Station, Store};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
const FILE_ERROR: &str = "RADIO_FAVORITES_UNAVAILABLE";
const NET_ERROR: &str = "RADIO_DIRECTORY_UNAVAILABLE";
#[derive(Default)]
struct Gate {
    state: Mutex<(bool, usize)>,
    changed: Condvar,
}
struct Activity(Arc<Gate>);
impl Drop for Activity {
    fn drop(&mut self) {
        if let Ok(mut value) = self.0.state.lock() {
            value.1 -= 1;
            self.0.changed.notify_all();
        }
    }
}
impl Gate {
    fn reserve(self: &Arc<Self>) -> Result<Activity, String> {
        let mut value = self.state.lock().map_err(|_| FILE_ERROR)?;
        if value.0 || value.1 >= 16 {
            return Err(FILE_ERROR.into());
        }
        value.1 += 1;
        Ok(Activity(self.clone()))
    }
    fn closing(&self) -> bool {
        self.state.lock().map_or(true, |value| value.0)
    }
}
#[derive(Clone)]
pub struct Controller {
    state: Arc<radio::State>,
    gate: Arc<Gate>,
    network: Arc<tokio::sync::Semaphore>,
}
impl Controller {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            state: Arc::new(radio::State {
                directory: Mutex::new(Default::default()),
                store: Mutex::new(Store::new(path)),
            }),
            gate: Arc::new(Gate::default()),
            network: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
    async fn file<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let activity = self.gate.reserve()?;
        let state = self.state.clone();
        let gate = self.gate.clone();
        tokio::task::spawn_blocking(move || {
            let _activity = activity;
            if gate.closing() {
                return Err(FILE_ERROR.into());
            }
            let mut store = state.store.lock().map_err(|_| FILE_ERROR)?;
            if gate.closing() {
                return Err(FILE_ERROR.into());
            }
            operation(&mut store)
        })
        .await
        .map_err(|_| FILE_ERROR.to_string())?
    }
    pub async fn favorites(
        &self,
        id: Option<String>,
        enabled: Option<bool>,
        name: Option<String>,
        url: Option<String>,
    ) -> Result<Vec<Station>, String> {
        self.file(move |store| match (id, enabled, name, url) {
            (Some(id), Some(enabled), None, None) => store.favorite(&id, enabled),
            (None, None, Some(name), Some(url)) => store.manual(&name, &url),
            (Some(id), None, Some(name), Some(url)) => store.edit_manual(&id, &name, &url),
            (None, None, None, None) => store.list(),
            _ => Err("RADIO_INVALID".into()),
        })
        .await
    }
    pub async fn station(&self, id: String) -> Result<Station, String> {
        self.file(move |store| store.station(&id)).await
    }
    pub async fn search(&self, query: Search) -> Result<Page, String> {
        let _slot = self
            .network
            .clone()
            .try_acquire_owned()
            .map_err(|_| NET_ERROR)?;
        let _activity = self.gate.reserve().map_err(|_| NET_ERROR)?;
        let result =
            tokio::time::timeout(Duration::from_secs(14), radio::search(&self.state, query))
                .await
                .map_err(|_| NET_ERROR)?;
        if self.gate.closing() {
            return Err(NET_ERROR.into());
        }
        result
    }
    pub async fn filters(&self) -> Result<Filters, String> {
        let _slot = self
            .network
            .clone()
            .try_acquire_owned()
            .map_err(|_| NET_ERROR)?;
        let _activity = self.gate.reserve().map_err(|_| NET_ERROR)?;
        let result = tokio::time::timeout(Duration::from_secs(14), radio::filters(&self.state))
            .await
            .map_err(|_| NET_ERROR)?;
        if self.gate.closing() {
            return Err(NET_ERROR.into());
        }
        result
    }
    // Original best-effort Radio Browser click is only for a confirmed explicit
    // playback. Failure never changes audio success; it writes no local state.
    pub async fn count_click(&self, id: &str) {
        let Ok(_slot) = self.network.clone().try_acquire_owned() else {
            return;
        };
        let Ok(_activity) = self.gate.reserve() else {
            return;
        };
        let _ =
            tokio::time::timeout(Duration::from_secs(5), radio::count_click(&self.state, id)).await;
    }
    pub fn begin_shutdown(&self) {
        if let Ok(mut value) = self.gate.state.lock() {
            value.0 = true;
            self.gate.changed.notify_all();
        }
        self.network.close();
    }
    pub fn shutdown(&self) -> Result<(), String> {
        self.begin_shutdown();
        let end = Instant::now() + Duration::from_secs(3);
        let mut value = self.gate.state.lock().map_err(|_| FILE_ERROR)?;
        while value.1 != 0 {
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(FILE_ERROR.into());
            }
            value = self
                .gate
                .changed
                .wait_timeout(value, remaining)
                .map_err(|_| FILE_ERROR)?
                .0;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn unused_path() -> std::path::PathBuf {
        let mut random = [0; 12];
        getrandom::fill(&mut random).unwrap();
        std::env::temp_dir()
            .join(format!("sonante-radio-host-{random:x?}"))
            .join("favorites.json")
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn construction_shutdown_and_closed_calls_never_create_files_or_start_network() {
        let path = unused_path();
        let controller = Controller::new(path.clone());
        assert!(!path.parent().unwrap().exists());
        controller.shutdown().unwrap();
        assert!(controller.favorites(None, None, None, None).await.is_err());
        assert!(controller.filters().await.is_err());
        assert!(!path.parent().unwrap().exists());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_caller_keeps_physical_file_capacity_until_worker_returns() {
        let controller = Controller::new(unused_path());
        let c = controller.clone();
        let (start_tx, start_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            c.file(move |_| {
                start_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .await
        });
        start_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(controller.gate.state.lock().unwrap().1, 1);
        let guards: Vec<_> = (0..15)
            .map(|_| controller.gate.reserve().unwrap())
            .collect();
        assert!(controller.gate.reserve().is_err());
        drop(guards);
        controller.begin_shutdown();
        assert!(controller.gate.reserve().is_err());
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller.gate.state.lock().unwrap().1 != 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        controller.shutdown().unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_network_slots_are_bounded_and_invalid_query_never_resolves_dns() {
        let controller = Controller::new(unused_path());
        let slots: Vec<_> = (0..4)
            .map(|_| controller.network.clone().try_acquire_owned().unwrap())
            .collect();
        let query = Search {
            name: "fixture".into(),
            country: String::new(),
            genre: String::new(),
            offset: 0,
        };
        assert!(controller.search(query).await.is_err());
        drop(slots);
        let bad = Search {
            name: String::new(),
            country: "INVALID".into(),
            genre: String::new(),
            offset: 0,
        };
        assert!(matches!(controller.search(bad).await,Err(code) if code=="RADIO_INVALID"));
        assert_eq!(controller.network.available_permits(), 4);
        assert_eq!(controller.gate.state.lock().unwrap().1, 0);
        controller.shutdown().unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manual_favorite_persists_across_controller_and_failed_write_keeps_confirmed_memory() {
        let path = unused_path();
        let controller = Controller::new(path.clone());
        let rows = controller
            .favorites(
                None,
                None,
                Some("Fixture".into()),
                Some("https://stream.fixture.invalid/live".into()),
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let fresh = Controller::new(path.clone());
        assert_eq!(fresh.favorites(None, None, None, None).await.unwrap(), rows);
        fresh.shutdown().unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(controller
            .favorites(
                None,
                None,
                Some("Other".into()),
                Some("https://stream.fixture.invalid/other".into())
            )
            .await
            .is_err());
        assert_eq!(
            controller.favorites(None, None, None, None).await.unwrap(),
            rows
        );
        let broken = Controller::new(path.clone());
        assert!(broken.favorites(None, None, None, None).await.is_err());
        broken.shutdown().unwrap();
        controller.shutdown().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
