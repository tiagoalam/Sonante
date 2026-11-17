//! Owned, bounded account operations. Dropping an IPC awaiter never releases
//! serialization while an admitted credential write is still running.
use super::{
    api::{self, Album, Credentials},
    connection::{Connection, Registry},
    favorites, store, CHANGED, INVALID, STORAGE, UNAVAILABLE,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

trait Vault: Send + Sync {
    fn load(&self, path: &Path) -> Result<Option<Credentials>, String>;
    fn save(&self, path: &Path, value: Option<Credentials>) -> Result<(), String>;
}
struct NativeVault;
impl Vault for NativeVault {
    fn load(&self, path: &Path) -> Result<Option<Credentials>, String> {
        store::load(path)
    }
    fn save(&self, path: &Path, value: Option<Credentials>) -> Result<(), String> {
        store::save(path, value)
    }
}
fn native_owner() -> Result<(), String> {
    crate::windows_instance::validate().map_err(|_| STORAGE.into())
}
#[derive(Default)]
struct Gate {
    state: Mutex<(bool, usize, bool)>,
    changed: Condvar,
}
#[derive(Clone)]
struct Lease(Arc<LeaseInner>);
struct LeaseInner(Arc<Gate>);
impl Drop for LeaseInner {
    fn drop(&mut self) {
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        s.1 -= 1;
        self.0.changed.notify_all();
    }
}
impl Gate {
    fn reserve(self: &Arc<Self>) -> Result<Lease, String> {
        let mut s = self.state.lock().map_err(|_| UNAVAILABLE)?;
        if s.0 {
            return Err(CHANGED.into());
        }
        if s.1 >= 16 {
            return Err(UNAVAILABLE.into());
        }
        s.1 += 1;
        Ok(Lease(Arc::new(LeaseInner(self.clone()))))
    }
    fn check(&self) -> Result<(), String> {
        if self.state.lock().map_err(|_| UNAVAILABLE)?.0 {
            Err(CHANGED.into())
        } else {
            Ok(())
        }
    }
    fn close(&self) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let first = !s.0;
        s.0 = true;
        self.changed.notify_all();
        first
    }
}
pub(crate) struct PreparedAccount(Credentials);
#[derive(Clone, Serialize)]
pub(crate) struct Status {
    pub(crate) connection_id: Option<String>,
    pub(crate) account_id: Option<String>,
    server: Option<String>,
    username: Option<String>,
}
impl Status {
    fn saved(credentials: Option<&Credentials>) -> Self {
        Self {
            connection_id: None,
            account_id: None,
            server: credentials.map(|c| c.server.clone()),
            username: credentials.map(|c| c.username.clone()),
        }
    }
}
struct Context {
    path: PathBuf,
    registry: Arc<Registry>,
    gate: Arc<Gate>,
    network: Arc<tokio::sync::Semaphore>,
    stop: tokio::sync::watch::Sender<bool>,
    vault: Arc<dyn Vault>,
    owner: fn() -> Result<(), String>,
    client: fn() -> Result<reqwest::Client, String>,
}
impl Context {
    fn check(&self) -> Result<(), String> {
        self.gate.check()?;
        (self.owner)()
    }
    async fn file<T: Send + 'static>(
        self: &Arc<Self>,
        lease: &Lease,
        work: impl FnOnce(&Context) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let context = self.clone();
        let physical = lease.clone();
        tokio::task::spawn_blocking(move || {
            let _physical = physical;
            context.check()?;
            let result = work(&context);
            context.check()?;
            result
        })
        .await
        .map_err(|_| STORAGE.to_string())?
    }
    async fn read<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        self.read_budget(Duration::from_secs(14), work).await
    }
    async fn read_budget<T>(&self, budget: Duration, work: impl std::future::Future<Output=Result<T,String>>) -> Result<T,String> {
        self.check()?;
        let _slot = self
            .network
            .clone()
            .try_acquire_owned()
            .map_err(|_| UNAVAILABLE)?;
        let mut stop = self.stop.subscribe();
        if *stop.borrow() {
            return Err(CHANGED.into());
        }
        let result = tokio::select! {biased;_ =stop.wait_for(|stopped|*stopped)=>Err(CHANGED.into()),result=tokio::time::timeout(budget,work)=>result.map_err(|_|UNAVAILABLE)?};
        self.check()?;
        result
    }
    async fn prepare(&self, mut credentials: Credentials) -> Result<Arc<Connection>, String> {
        credentials.connection_id = api::random()?;
        Connection::open_with_client(credentials, (self.client)()?).await
    }
    fn status(&self) -> Result<Status, String> {
        let c = self.registry.selected()?;
        Ok(match c {
            Some(c) => Status {
                connection_id: Some(c.credentials.connection_id.clone()),
                account_id: Some(c.account()),
                server: Some(c.credentials.server.clone()),
                username: Some(c.credentials.username.clone()),
            },
            None => Status::saved(None),
        })
    }
}
struct Worker(Arc<Context>);
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.gate.close();
        self.0.stop.send_replace(true);
        self.0.network.close();
        self.0.registry.retire_all();
        let mut s = self.0.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        s.2 = false;
        self.0.gate.changed.notify_all();
    }
}
enum Operation {
    Status,
    Enabled(bool),
    ConnectPrepared(PreparedAccount),
    Disconnect,
    Favorites {
        connection_id: String,
        id: Option<String>,
        enabled: Option<bool>,
    },
    Shutdown,
}
enum Response {
    Status(Status),
    Albums(Vec<Album>),
}
struct Message {
    operation: Operation,
    reply: tokio::sync::oneshot::Sender<Result<Response, String>>,
    lease: Option<Lease>,
}
struct AccountState {
    enabled: bool,
    loaded: bool,
}
async fn status(
    context: &Arc<Context>,
    state: &mut AccountState,
    lease: &Lease,
) -> Result<Status, String> {
    if state.enabled && state.loaded {
        return context.status();
    }
    let credentials = context.file(lease, |c| c.vault.load(&c.path)).await?;
    if !state.enabled {
        return Ok(Status::saved(credentials.as_ref()));
    }
    let next = match credentials {
        Some(c) => Some(context.prepare(c).await?),
        None => None,
    };
    context.check()?;
    context
        .file(lease, move |c| c.registry.replace(next))
        .await?;
    state.loaded = true;
    context.status()
}
async fn execute(
    context: &Arc<Context>,
    state: &mut AccountState,
    lease: &Lease,
    operation: Operation,
) -> Result<Response, String> {
    context.check()?;
    match operation {
        Operation::Status => Ok(Response::Status(status(context, state, lease).await?)),
        Operation::Enabled(enabled) => {
            if !enabled {
                context.file(lease, |c| c.registry.replace(None)).await?;
                state.enabled = false;
                state.loaded = false;
                return Ok(Response::Status(Status::saved(None)));
            }
            if state.enabled {
                return Ok(Response::Status(status(context, state, lease).await?));
            }
            let credentials = context.file(lease, |c| c.vault.load(&c.path)).await?;
            let next = match credentials {
                Some(c) => Some(context.prepare(c).await?),
                None => None,
            };
            context.check()?;
            context
                .file(lease, move |c| c.registry.replace(next))
                .await?;
            state.enabled = true;
            state.loaded = true;
            Ok(Response::Status(context.status()?))
        }
        Operation::ConnectPrepared(prepared) => {
            let credentials=prepared.0;
            credentials.validate()?;
            let next = if state.enabled {
                Some(context.prepare(credentials.clone()).await?)
            } else {
                None
            };
            let saved = Status::saved(Some(&credentials));
            context
                .file(lease, move |c| {
                    if let Err(error) = c.vault.save(&c.path, Some(credentials)) {
                        if let Some(next) = next {
                            next.retire();
                        }
                        return Err(error);
                    }
                    c.check()?;
                    c.registry.replace(next)
                })
                .await?;
            state.loaded = true;
            Ok(Response::Status(if state.enabled {
                context.status()?
            } else {
                saved
            }))
        }
        Operation::Disconnect => {
            context
                .file(lease, |c| {
                    c.vault.save(&c.path, None)?;
                    c.check()?;
                    c.registry.replace(None)
                })
                .await?;
            state.loaded = true;
            Ok(Response::Status(Status::saved(None)))
        }
        Operation::Favorites {
            connection_id,
            id,
            enabled,
        } => {
            if id.as_deref().is_some_and(|id| !api::id(id)) || id.is_some() != enabled.is_some() {
                return Err(INVALID.into());
            }
            let c = context.registry.current(&connection_id)?;
            let account = c.account();
            let album = if enabled == Some(true) {
                Some(
                    context
                        .read(
                            context
                                .registry
                                .album(connection_id.clone(), id.clone().ok_or(INVALID)?),
                        )
                        .await?
                        .album,
                )
            } else {
                None
            };
            context.registry.with_current(&c, || Ok(()))?;
            let albums = context
                .file(lease, move |c| {
                    let path = c.path.with_file_name("windows-navidrome-favorites-v1.json");
                    match id {
                        Some(id) => favorites::set(&path, &account, &id, album),
                        None => favorites::read(&path, &account),
                    }
                })
                .await?;
            context.registry.with_current(&c, || Ok(()))?;
            Ok(Response::Albums(albums))
        }
        Operation::Shutdown => Err(CHANGED.into()),
    }
}
#[derive(Clone)]
pub(crate) struct Controller {
    context: Arc<Context>,
    sender: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<Message>>>>,
}
impl Controller {
    pub(crate) fn new(path: PathBuf) -> Result<Self, String> {
        Self::with_parts(path, Arc::new(NativeVault), native_owner, api::client)
    }
    fn with_parts(
        path: PathBuf,
        vault: Arc<dyn Vault>,
        owner: fn() -> Result<(), String>,
        client: fn() -> Result<reqwest::Client, String>,
    ) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err(STORAGE.into());
        }
        let (stop, _) = tokio::sync::watch::channel(false);
        Ok(Self {
            context: Arc::new(Context {
                path,
                registry: Arc::new(Registry::default()),
                gate: Arc::new(Gate::default()),
                network: Arc::new(tokio::sync::Semaphore::new(4)),
                stop,
                vault,
                owner,
                client,
            }),
            sender: Arc::new(Mutex::new(None)),
        })
    }
    fn sender(&self) -> Result<tokio::sync::mpsc::UnboundedSender<Message>, String> {
        let mut sender = self.sender.lock().map_err(|_| UNAVAILABLE)?;
        self.context.gate.check()?;
        if let Some(sender) = sender.as_ref() {
            return Ok(sender.clone());
        }
        let handle = tokio::runtime::Handle::try_current().map_err(|_| UNAVAILABLE)?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        {
            let mut s = self.context.gate.state.lock().map_err(|_| UNAVAILABLE)?;
            if s.0 {
                return Err(CHANGED.into());
            }
            s.2 = true;
        }
        let context = self.context.clone();
        // Poll account work on one owned, lazy OS worker. In particular its
        // terminal Registry cleanup may wait for a synchronous queue effect
        // lease without blocking any Tokio executor thread.
        let spawned = std::thread::Builder::new().name("sonante-windows-navidrome".into()).spawn(move || {
            let _worker = Worker(context.clone());
            handle.block_on(async move {
            let mut state = AccountState {
                enabled: false,
                loaded: false,
            };
            while let Some(message) = rx.recv().await {
                let Message {
                    operation,
                    reply,
                    lease,
                } = message;
                if matches!(operation, Operation::Shutdown) {
                    let _ = reply.send(Ok(Response::Status(Status::saved(None))));
                    break;
                }
                let result = match lease.as_ref() {
                    Some(lease) => execute(&context, &mut state, lease, operation).await,
                    None => Err(UNAVAILABLE.into()),
                };
                let _ = reply.send(result);
                drop(lease);
            }
            });
        });
        if spawned.is_err() {
            let mut state = self.context.gate.state.lock().map_err(|_| UNAVAILABLE)?;
            state.2 = false;
            self.context.gate.changed.notify_all();
            return Err(UNAVAILABLE.into());
        }
        *sender = Some(tx.clone());
        Ok(tx)
    }
    async fn call(&self, operation: Operation) -> Result<Response, String> {
        let lease = self.context.gate.reserve()?;
        let sender = self.sender()?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        sender
            .send(Message {
                operation,
                reply: tx,
                lease: Some(lease),
            })
            .map_err(|_| UNAVAILABLE)?;
        // The worker owns the command/lease through the actual ACK. Dropping
        // this receiver never aborts a started store or unlocks the next write.
        rx.await.map_err(|_| UNAVAILABLE.to_string())?
    }
    async fn status_call(&self, op: Operation) -> Result<Status, String> {
        match self.call(op).await? {
            Response::Status(s) => Ok(s),
            _ => Err(UNAVAILABLE.into()),
        }
    }
    pub(crate) async fn status(&self) -> Result<Status, String> {
        self.status_call(Operation::Status).await
    }
    pub(crate) async fn apply_enabled_from_config(&self, enabled: bool) -> Result<Status, String> {
        self.status_call(Operation::Enabled(enabled)).await
    }
    pub(crate) async fn prepare_account(&self,server:String,username:String,password:String)->Result<PreparedAccount,String>{
        let credentials=Credentials{connection_id:api::random()?,server:api::server(&server)?.to_string(),username:username.trim().into(),password};
        credentials.validate()?;
        let client=(self.context.client)()?;
        self.read(api::request(&client,&credentials,"ping",&[])).await?;
        Ok(PreparedAccount(credentials))
    }
    pub(crate) async fn commit_account(&self,prepared:PreparedAccount)->Result<Status,String>{self.status_call(Operation::ConnectPrepared(prepared)).await}
    pub(crate) async fn connect(&self,server:String,username:String,password:String)->Result<Status,String>{
        let prepared=self.prepare_account(server,username,password).await?;self.commit_account(prepared).await
    }
    pub(crate) async fn disconnect(&self) -> Result<Status, String> {
        self.status_call(Operation::Disconnect).await
    }
    pub(crate) async fn favorites(
        &self,
        connection_id: String,
        id: Option<String>,
        enabled: Option<bool>,
    ) -> Result<Vec<Album>, String> {
        match self
            .call(Operation::Favorites {
                connection_id,
                id,
                enabled,
            })
            .await?
        {
            Response::Albums(a) => Ok(a),
            _ => Err(UNAVAILABLE.into()),
        }
    }
    pub(crate) fn registry(&self) -> Arc<Registry> {
        self.context.registry.clone()
    }
    pub(crate) async fn read<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let _lease = self.context.gate.reserve()?;
        self.context.read(work).await
    }
    pub(crate) async fn catalog_read<T>(&self,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{
        let _lease=self.context.gate.reserve()?;
        self.context.read_budget(Duration::from_secs(120),work).await
    }
    pub(crate) fn begin_shutdown(&self) {
        if !self.context.gate.close() {
            return;
        }
        self.context.stop.send_replace(true);
        self.context.network.close();
        if let Ok(sender) = self.sender.lock() {
            if let Some(sender) = sender.as_ref() {
                let (reply, _) = tokio::sync::oneshot::channel();
                let _ = sender.send(Message {
                    operation: Operation::Shutdown,
                    reply,
                    lease: None,
                });
            }
        }
    }
    pub(crate) fn shutdown(&self) -> Result<(), String> {
        self.begin_shutdown();
        let end = Instant::now() + Duration::from_secs(3);
        let mut s = self.context.gate.state.lock().map_err(|_| UNAVAILABLE)?;
        while s.1 != 0 || s.2 {
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(UNAVAILABLE.into());
            }
            s = self
                .context
                .gate
                .changed
                .wait_timeout(s, remaining)
                .map_err(|_| UNAVAILABLE)?
                .0;
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;

#[cfg(test)]
impl Controller {
 pub(crate) fn smart_test_host(path:PathBuf,credentials:Credentials)->Self {
  struct Memory(Mutex<Option<Credentials>>);
  impl Vault for Memory {
   fn load(&self,_:&Path)->Result<Option<Credentials>,String>{Ok(self.0.lock().unwrap().clone())}
   fn save(&self,_:&Path,value:Option<Credentials>)->Result<(),String>{*self.0.lock().unwrap()=value;Ok(())}
  }
  fn own()->Result<(),String>{Ok(())}
  fn http()->Result<reqwest::Client,String>{reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|UNAVAILABLE.into())}
  Self::with_parts(path,Arc::new(Memory(Mutex::new(Some(credentials)))),own,http).unwrap()
 }
}
