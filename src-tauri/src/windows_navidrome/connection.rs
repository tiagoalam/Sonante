//! Explicit source connections only; construction is not wired to app startup.
use super::{api, proxy, CHANGED, UNAVAILABLE};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock,
};

pub(super) struct Connection {
    pub(super) credentials: api::Credentials,
    pub(super) client: reqwest::Client,
    port: Mutex<Option<u16>>,
    pub(super) secret: String,
    pub(super) covers: tokio::sync::Semaphore,
    pub(super) details: Mutex<std::collections::VecDeque<super::catalog::Detail>>,
    pub(super) smart_catalog: tokio::sync::Mutex<Option<super::smart::CatalogSnapshot>>,
    live: Arc<AtomicBool>,
    pub(super) stop: tokio::sync::watch::Sender<bool>,
}
impl Connection {
    pub(super) async fn open(mut credentials: api::Credentials) -> Result<Arc<Self>, String> {
        credentials.validate()?;
        // Stored credentials identify the account, not a live playback generation.
        // Reopening must invalidate every previously issued in-process reference.
        credentials.connection_id = api::random()?;
        Self::open_with_client(credentials, api::client()?).await
    }
    pub(super) async fn open_with_client(
        mut credentials: api::Credentials,
        client: reqwest::Client,
    ) -> Result<Arc<Self>, String> {
        credentials.validate()?;
        credentials.server = api::server(&credentials.server)?.to_string();
        let (stop, _) = tokio::sync::watch::channel(false);
        let connection = Arc::new(Self {
            credentials,
            client,
            port: Mutex::new(None),
            secret: api::random()?,
            covers: tokio::sync::Semaphore::new(3),
            details: Mutex::new(std::collections::VecDeque::new()),
            smart_catalog: tokio::sync::Mutex::new(None),
            live: Arc::new(AtomicBool::new(true)),
            stop,
        });
        Ok(connection)
    }
    #[cfg(test)]
    pub(super) fn playback_listener_started(&self) -> bool {
        self.port.lock().unwrap().is_some()
    }
    pub(super) fn health_signal(&self)->Arc<AtomicBool>{self.live.clone()}
    pub(super) fn active(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
    pub(super) fn retire(&self) {
        self.live.store(false, Ordering::Release);
        self.stop.send_replace(true);
    }
    pub(super) fn stream(self: &Arc<Self>, song: &str) -> Result<String, String> {
        if !self.active() || !api::id(song) {
            return Err(CHANGED.into());
        }
        let port = {
            let mut current = self.port.lock().map_err(|_| UNAVAILABLE)?;
            if !self.active() {
                return Err(CHANGED.into());
            }
            if let Some(port) = *current {
                port
            } else {
                // Reading the account/catalog, restoring credentials and startup
                // never bind a playback server. Only explicit stream preparation
                // may create the bridge, while its async runtime is available.
                tokio::runtime::Handle::try_current().map_err(|_| UNAVAILABLE)?;
                let (listener, port) = proxy::bind()?;
                let listener =
                    tokio::net::TcpListener::from_std(listener).map_err(|_| UNAVAILABLE)?;
                proxy::start(listener, Arc::downgrade(self), self.stop.subscribe());
                *current = Some(port);
                port
            }
        };
        if !self.active() {
            return Err(CHANGED.into());
        }
        Ok(format!(
            "http://127.0.0.1:{port}/nav/{}/{}",
            self.secret, song
        ))
    }
    pub(super) fn account(&self) -> String {
        // Same normalized server/username scope as the original favorites module.
        let identity = serde_json::to_vec(&[
            self.credentials.server.as_str(),
            self.credentials.username.as_str(),
        ])
        .expect("string array");
        format!("{:x}", Sha256::digest(identity))
    }
    pub(super) async fn request(
        &self,
        endpoint: &str,
        params: &[(&str, String)],
    ) -> Result<serde_json::Value, String> {
        if !self.active() {
            return Err(CHANGED.into());
        }
        let mut stop = self.stop.subscribe();
        let result = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return Err(CHANGED.into()),
            result = api::request(&self.client, &self.credentials, endpoint, params) => result
        };
        if !self.active() {
            return Err(CHANGED.into());
        }
        result
    }
    pub(super) fn remember(&self, detail: &super::catalog::Detail) -> Result<(), String> {
        if detail.songs.len() > 500 {
            return Ok(());
        }
        let mut cache = self.details.lock().map_err(|_| UNAVAILABLE)?;
        cache.retain(|old| old.album.id != detail.album.id);
        cache.push_back(detail.clone());
        while cache.len() > 8
            || cache
                .iter()
                .map(super::catalog::Detail::size)
                .sum::<usize>()
                > 4 * 1024 * 1024
        {
            cache.pop_front();
        }
        Ok(())
    }
    pub(super) fn prepared(&self, album: &str) -> Result<super::catalog::Detail, String> {
        if !self.active() {
            return Err(CHANGED.into());
        }
        self.details
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .iter()
            .find(|detail| detail.album.id == album)
            .cloned()
            .ok_or_else(|| "NAVIDROME_DETAILS_EXPIRED".into())
    }
    pub(super) async fn while_active<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        if !self.active() {
            return Err(CHANGED.into());
        }
        let mut stop = self.stop.subscribe();
        let result = tokio::select! {
            biased;
            _=stop.wait_for(|stopped|*stopped)=>return Err(CHANGED.into()),
            result=work=>result
        };
        if !self.active() {
            return Err(CHANGED.into());
        }
        result
    }
    pub(super) async fn request_counted(
        &self,
        endpoint: &str,
        params: &[(&str, String)],
    ) -> Result<(serde_json::Value, Option<usize>), String> {
        self.while_active(api::request_counted(
            &self.client,
            &self.credentials,
            endpoint,
            params,
        ))
        .await
    }
    #[cfg(test)]
    pub(super) async fn fixture(credentials: api::Credentials) -> Arc<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(2))
            .read_timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        Self::open_with_client(credentials, client).await.unwrap()
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Serialize the final queue effect with replacement, just as the original
/// with_bound_tracks contract does. No guard is held across async HTTP work.
#[derive(Default)]
pub(crate) struct Registry {
    current: RwLock<Option<Arc<Connection>>>,
}
impl Registry {
    pub(super) fn replace(&self, next: Option<Arc<Connection>>) -> Result<(), String> {
        let mut current = self.current.write().map_err(|_| UNAVAILABLE)?;
        if let Some(next) = &next {
            if !next.active() {
                return Err(CHANGED.into());
            }
            if let Some(old) = current.as_ref() {
                if Arc::ptr_eq(old, next) {
                    return Ok(());
                }
                // A generation identifier must not be recycled within the process.
                if old.credentials.connection_id == next.credentials.connection_id {
                    return Err(CHANGED.into());
                }
            }
        }
        if let Some(old) = std::mem::replace(&mut *current, next) {
            old.retire();
        }
        Ok(())
    }
    pub(super) fn selected(&self) -> Result<Option<Arc<Connection>>, String> {
        Ok(self
            .current
            .try_read()
            .map_err(|_| UNAVAILABLE)?
            .as_ref()
            .filter(|c| c.active())
            .cloned())
    }
    pub(super) fn retire_all(&self) {
        let mut current = self.current.write().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = current.take() {
            old.retire();
        }
    }
    pub(super) fn current(&self, id: &str) -> Result<Arc<Connection>, String> {
        self.current
            .try_read()
            .map_err(|_| UNAVAILABLE)?
            .as_ref()
            .filter(|c| c.active() && c.credentials.connection_id == id)
            .cloned()
            .ok_or(CHANGED.into())
    }
    pub(super) fn for_account(&self, account: &str) -> Result<Arc<Connection>, String> {
        if !crate::navidrome::valid_account(account) {
            return Err(CHANGED.into());
        }
        self.current
            .try_read()
            .map_err(|_| UNAVAILABLE)?
            .as_ref()
            .filter(|c| c.active() && c.account() == account)
            .cloned()
            .ok_or(CHANGED.into())
    }
    pub(super) fn with_current<T>(
        &self,
        expected: &Arc<Connection>,
        effect: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let guard = self.current.try_read().map_err(|_| UNAVAILABLE)?;
        let current = guard
            .as_ref()
            .filter(|c| c.active() && Arc::ptr_eq(c, expected))
            .ok_or(CHANGED)?;
        if current.credentials.connection_id != expected.credentials.connection_id {
            return Err(CHANGED.into());
        }
        let result = effect();
        drop(guard);
        result
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        // Even if poisoned, an abandoned registry must retire its current proxy.
        let current = self.current.get_mut().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = current.take() {
            old.retire();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn credentials(id: &str) -> api::Credentials {
        api::Credentials {
            connection_id: id.into(),
            server: "http://127.0.0.1:9/".into(),
            username: "fixture".into(),
            password: "PRIVATE".into(),
        }
    }
    #[tokio::test]
    async fn pcm_signal_does_not_start_a_listener_or_retain_credentials_after_drop() {
        let connection = Connection::fixture(credentials("health-fixture")).await;
        let weak = Arc::downgrade(&connection);
        let watch = crate::windows_mpd::liveness::Watch::new(vec![crate::windows_mpd::liveness::Signal::remote(connection.health_signal())]).unwrap();
        assert!(watch.healthy());
        assert!(!connection.playback_listener_started());
        let stop = connection.stop.subscribe();
        drop(connection);
        assert!(!watch.healthy());
        assert!(weak.upgrade().is_none());
        assert!(*stop.borrow());
    }
    #[tokio::test]
    async fn replacement_is_leased_through_effects_and_drop_retires_even_retained_connections() {
        let first = Connection::fixture(credentials("first")).await;
        let second = Connection::fixture(credentials("second")).await;
        let registry = Registry::default();
        registry.replace(Some(first.clone())).unwrap();
        registry
            .with_current(&first, || {
                assert!(registry.current.try_write().is_err());
                assert!(registry.current.try_read().is_ok());
                Ok(())
            })
            .unwrap();
        registry.replace(Some(first.clone())).unwrap();
        assert!(first.active());
        registry.replace(Some(second.clone())).unwrap();
        assert!(!first.active());
        assert!(registry
            .with_current::<()>(&first, || panic!("retired lease reached effect"))
            .is_err());
        assert!(registry.current("first").is_err());
        assert!(registry.current("second").is_ok());
        let reused = Connection::fixture(credentials("second")).await;
        assert!(registry.replace(Some(reused.clone())).is_err());
        assert!(second.active());
        reused.retire();
        drop(registry);
        assert!(!second.active());
        assert!(*second.stop.subscribe().borrow());
    }
    #[tokio::test]
    async fn replacing_a_poisoned_registry_fails_closed_and_drop_still_retires() {
        let connection = Connection::fixture(credentials("first")).await;
        let registry = Registry::default();
        registry.replace(Some(connection.clone())).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = registry.current.write().unwrap();
            panic!("fixture poison");
        }));
        assert!(result.is_err());
        assert!(registry.current("first").is_err());
        assert!(registry.replace(None).is_err());
        drop(registry);
        assert!(!connection.active());
    }
}

#[cfg(test)]
mod quiet_startup_tests {
    use super::*;
    fn credentials() -> api::Credentials {
        api::Credentials {
            connection_id: "OLD".into(),
            server: "http://127.0.0.1:9/".into(),
            username: "fixture".into(),
            password: "PRIVATE".into(),
        }
    }
    #[tokio::test]
    async fn opening_and_retiring_account_never_create_a_media_listener() {
        let c = Connection::fixture(credentials()).await;
        assert!(c.port.lock().unwrap().is_none());
        let registry = Registry::default();
        registry.replace(Some(c.clone())).unwrap();
        assert_eq!(registry.current("OLD").unwrap().account(), c.account());
        assert!(c.port.lock().unwrap().is_none());
        assert!(c.stream("../invalid").is_err());
        assert!(c.port.lock().unwrap().is_none());
        registry.replace(None).unwrap();
        assert!(c.port.lock().unwrap().is_none());
        assert!(c.stream("song").is_err());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_stream_initializes_once_and_worker_revalidation_needs_no_runtime() {
        let c = Connection::fixture(credentials()).await;
        assert!(c.port.lock().unwrap().is_none());
        let first = c.stream("song").unwrap();
        let port = c.port.lock().unwrap().unwrap();
        let other = c.clone();
        let second = std::thread::spawn(move || {
            assert!(tokio::runtime::Handle::try_current().is_err());
            other.stream("song")
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(*c.port.lock().unwrap(), Some(port));
        let socket = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
        drop(socket);
        c.retire();
        assert!(c.stream("song").is_err());
    }
}
