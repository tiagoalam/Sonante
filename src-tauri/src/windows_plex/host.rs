//! Owned Windows Plex account operations. Renderer cancellation cannot release
//! admission or publish a newer account before the admitted store work returns.
use super::{Client,Settings,store,CHANGED,INVALID,UNAVAILABLE};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,RwLock,Condvar,mpsc},time::{Duration,Instant}};
const STORAGE:&str="PLEX_STORAGE";
trait Vault:Send+Sync {
 fn load(&self,path:&Path)->Result<Option<store::Account>,String>;
 fn save(&self,path:&Path,account:Option<store::Account>)->Result<(),String>;
}
struct NativeVault;
impl Vault for NativeVault {
 fn load(&self,path:&Path)->Result<Option<store::Account>,String>{store::load(path)}
 fn save(&self,path:&Path,account:Option<store::Account>)->Result<(),String>{store::save(path,account)}
}
fn owner()->Result<(),String>{crate::windows_instance::validate().map_err(|_|STORAGE.into())}
#[derive(Default)]struct Gate{state:Mutex<(bool,usize,bool,usize)>,changed:Condvar}
// Total live work is drained together; artwork never consumes control admission.
struct Activity(Arc<Gate>,bool);
impl Drop for Activity{fn drop(&mut self){let mut s=self.0.state.lock().unwrap_or_else(|e|e.into_inner());s.1-=1;if self.1{s.3-=1;}self.0.changed.notify_all();}}
impl Gate {
 fn reserve(self:&Arc<Self>)->Result<Activity,String>{self.reserve_kind(false)}
 fn reserve_image(self:&Arc<Self>)->Result<Activity,String>{self.reserve_kind(true)}
 fn reserve_kind(self:&Arc<Self>,image:bool)->Result<Activity,String>{let mut s=self.state.lock().map_err(|_|UNAVAILABLE)?;if s.0{return Err(CHANGED.into());}if (image&&s.3>=12)||(!image&&s.1-s.3>=16){return Err(UNAVAILABLE.into());}s.1+=1;if image{s.3+=1;}Ok(Activity(self.clone(),image))}
 fn check(&self)->Result<(),String>{if self.state.lock().map_err(|_|UNAVAILABLE)?.0{Err(CHANGED.into())}else{Ok(())}}
 fn close(&self){let mut s=self.state.lock().unwrap_or_else(|e|e.into_inner());s.0=true;self.changed.notify_all();}
}
#[derive(Default)]pub(super) struct Registry{current:RwLock<Option<Arc<Binding>>>}
pub(super) struct Binding {
 pub(super) client:Client,
 pub(super) settings:Settings,
 pub(super) secret:String,
 pub(super) generation:String,
 pub(super) stop:tokio::sync::watch::Sender<bool>,
 pub(super) stream_http:reqwest::Client,
 port:Mutex<Option<u16>>,
 parts:Mutex<std::collections::HashMap<String,std::sync::Weak<Part>>>,
 live:Arc<std::sync::atomic::AtomicBool>,
}
impl Binding {
 fn new(settings:Settings)->Result<Arc<Self>,String>{
  let client=Client::new(&settings)?;
  let stream_http=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(3)).read_timeout(Duration::from_secs(20)).build().map_err(|_|UNAVAILABLE)?;
  Ok(Arc::new(Self{stop:client.stop.clone(),client,settings,secret:random()?,generation:random()?,stream_http,port:Mutex::new(None),parts:Mutex::new(Default::default()),live:Arc::new(std::sync::atomic::AtomicBool::new(true))}))
 }
 pub(super) fn health_signal(&self)->Arc<std::sync::atomic::AtomicBool>{self.live.clone()}
 pub(super) fn active(&self)->bool{self.live.load(std::sync::atomic::Ordering::Acquire)}
 fn retire(&self){self.live.store(false,std::sync::atomic::Ordering::Release);self.client.shutdown();}
}

fn random()->Result<String,String>{let mut bytes=[0u8;24];getrandom::fill(&mut bytes).map_err(|_|UNAVAILABLE)?;Ok(bytes.iter().map(|b|format!("{b:02x}")).collect())}
pub(super) struct Part{pub(super) source:crate::windows_mpd::queue_v3::Source,pub(super) uri:String}
impl Binding {
 pub(super) fn part(&self,id:&str)->Result<Option<Arc<Part>>,String>{let mut parts=self.parts.lock().map_err(|_|UNAVAILABLE)?;parts.retain(|_,part|part.strong_count()!=0);Ok(parts.get(id).and_then(std::sync::Weak::upgrade))}
 pub(super) fn admit(self:&Arc<Self>,source:crate::windows_mpd::queue_v3::Source)->Result<Arc<Part>,String>{
  if !self.active(){return Err(CHANGED.into());}
  let crate::windows_mpd::queue_v3::Source::Plex{server_id,rating_key,part_key}=&source else{return Err(INVALID.into())};
  super::core::content::validate_reference(server_id,rating_key,part_key)?;
  if self.client.manager.selected_server_id().as_ref()!=Some(server_id){return Err(CHANGED.into());}
  let mut parts=self.parts.lock().map_err(|_|UNAVAILABLE)?;parts.retain(|_,part|part.strong_count()!=0);if parts.len()>=6000{return Err(UNAVAILABLE.into());}
  let mut port=self.port.lock().map_err(|_|UNAVAILABLE)?;
  let port=if let Some(port)=*port{port}else{
   tokio::runtime::Handle::try_current().map_err(|_|UNAVAILABLE)?;
   let(listener,selected)=super::proxy::bind()?;let listener=tokio::net::TcpListener::from_std(listener).map_err(|_|UNAVAILABLE)?;
   super::proxy::start(listener,Arc::downgrade(self),self.stop.subscribe());*port=Some(selected);selected
  };
  let id=random()?;let part=Arc::new(Part{source,uri:format!("http://127.0.0.1:{port}/plex/{}/{id}",self.secret)});parts.insert(id,Arc::downgrade(&part));Ok(part)
 }
}
impl Drop for Binding{fn drop(&mut self){self.retire();}}
impl Registry {
 pub(super) fn selected(&self)->Result<Arc<Binding>,String>{self.current.try_read().map_err(|_|UNAVAILABLE)?.as_ref().filter(|c|c.active()).cloned().ok_or(CHANGED.into())}
 pub(super) fn with_current<T>(&self,expected:&Arc<Binding>,effect:impl FnOnce()->Result<T,String>)->Result<T,String>{let guard=self.current.try_read().map_err(|_|UNAVAILABLE)?;let current=guard.as_ref().filter(|c|c.active()&&Arc::ptr_eq(c,expected)).ok_or(CHANGED)?;let _current=current;let result=effect();drop(guard);result}
 fn replace(&self,next:Option<Arc<Binding>>,commit:impl FnOnce()->Result<(),String>)->Result<(),String>{
  let mut current=self.current.write().map_err(|_|UNAVAILABLE)?;
  // No async work under this guard. An atomic vault failure preserves the old
  // live binding; terminal cleanup and this wait run only on the owned OS worker.
  commit()?;
  if let Some(old)=std::mem::replace(&mut *current,next){old.retire();}Ok(())
 }
 fn retire_all(&self){let mut current=self.current.write().unwrap_or_else(|e|e.into_inner());if let Some(old)=current.take(){old.retire();}}
}
impl Drop for Registry{fn drop(&mut self){if let Some(old)=self.current.get_mut().unwrap_or_else(|e|e.into_inner()).take(){old.retire();}}}
struct Context {
 path:PathBuf,registry:Arc<Registry>,gate:Arc<Gate>,network:Arc<tokio::sync::Semaphore>,artwork:Arc<tokio::sync::Semaphore>,
 stop:tokio::sync::watch::Sender<bool>,vault:Arc<dyn Vault>,owner:fn()->Result<(),String>,
}
impl Context {
 fn check(&self)->Result<(),String>{self.gate.check()?;(self.owner)()}
 async fn read<T>(&self,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{self.read_budget(Duration::from_secs(14),work).await}
 async fn read_budget<T>(&self,budget:Duration,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{
  self.check()?;let _slot=self.network.clone().try_acquire_owned().map_err(|_|UNAVAILABLE)?;let mut stop=self.stop.subscribe();
  let result=tokio::select!{biased;_=stop.wait_for(|closed|*closed)=>Err(CHANGED.into()),value=tokio::time::timeout(budget,work)=>value.map_err(|_|UNAVAILABLE)?};self.check()?;result
 }
}
pub(crate) struct PreparedAccount(Option<store::Account>);
enum Operation{Status,Enabled(bool),Commit(PreparedAccount),Configure(bool,Option<PreparedAccount>),Shutdown}
struct Message{operation:Operation,reply:tokio::sync::oneshot::Sender<Result<Settings,String>>,activity:Option<Activity>}
struct State{enabled:bool,loaded:bool,account:Option<store::Account>}
impl State {
 fn settings(&self)->Settings{self.account.as_ref().map(|a|a.settings(self.enabled)).unwrap_or_default()}
 fn load(&mut self,context:&Context)->Result<(),String>{if !self.loaded{let account=context.vault.load(&context.path)?;context.check()?;self.account=account;self.loaded=true;}Ok(())}
 fn next(&self,account:Option<&store::Account>,enabled:bool)->Result<Option<Arc<Binding>>,String>{account.filter(|_|enabled).map(|a|Binding::new(a.settings(true))).transpose()}
 fn configure(&mut self,context:&Context,enabled:bool,prepared:Option<PreparedAccount>)->Result<Settings,String>{
  let changed=prepared.is_some();let account=prepared.map(|p|p.0).unwrap_or_else(||self.account.clone());
  if changed || self.enabled!=enabled || (enabled&&account.is_some()&&context.registry.selected().is_err()) {
   let next=self.next(account.as_ref(),enabled)?;
   context.registry.replace(next,||{context.check()?;if changed{context.vault.save(&context.path,account.clone())}else{Ok(())}})?;
   self.account=account;self.enabled=enabled;
  }Ok(self.settings())
 }
 fn execute(&mut self,context:&Context,operation:Operation)->Result<Settings,String>{
  context.check()?;self.load(context)?;
  match operation {
   Operation::Status=>Ok(self.settings()),
   Operation::Enabled(enabled)=>self.configure(context,enabled,None),
   Operation::Commit(prepared)=>self.configure(context,self.enabled,Some(prepared)),
   Operation::Configure(enabled,prepared)=>self.configure(context,enabled,prepared),
   Operation::Shutdown=>Err(CHANGED.into()),
  }
 }
}
struct Worker(Arc<Context>);
impl Drop for Worker{fn drop(&mut self){self.0.gate.close();self.0.stop.send_replace(true);self.0.network.close();self.0.artwork.close();self.0.registry.retire_all();let mut s=self.0.gate.state.lock().unwrap_or_else(|e|e.into_inner());s.2=false;self.0.gate.changed.notify_all();}}
#[derive(Clone)]pub(crate) struct Controller{context:Arc<Context>,sender:Arc<Mutex<Option<mpsc::Sender<Message>>>>}
impl Controller {
 pub(crate) fn new(path:PathBuf)->Result<Self,String>{Self::with_parts(path,Arc::new(NativeVault),owner)}
 fn with_parts(path:PathBuf,vault:Arc<dyn Vault>,owner:fn()->Result<(),String>)->Result<Self,String>{
  if !path.is_absolute(){return Err(STORAGE.into());}let(stop,_)=tokio::sync::watch::channel(false);
  Ok(Self{context:Arc::new(Context{path,registry:Arc::new(Registry::default()),gate:Arc::new(Gate::default()),network:Arc::new(tokio::sync::Semaphore::new(4)),artwork:Arc::new(tokio::sync::Semaphore::new(2)),stop,vault,owner}),sender:Arc::new(Mutex::new(None))})
 }
 fn sender(&self)->Result<mpsc::Sender<Message>,String>{
  let mut sender=self.sender.lock().map_err(|_|UNAVAILABLE)?;self.context.check()?;if let Some(sender)=sender.as_ref(){return Ok(sender.clone());}
  let(tx,rx)=mpsc::channel::<Message>();{let mut state=self.context.gate.state.lock().map_err(|_|UNAVAILABLE)?;if state.0{return Err(CHANGED.into());}state.2=true;}
  let context=self.context.clone();let worker=std::thread::Builder::new().name("sonante-windows-plex".into()).spawn(move||{
   let _worker=Worker(context.clone());let mut state=State{enabled:false,loaded:false,account:None};
   while let Ok(message)=rx.recv(){let Message{operation,reply,activity}=message;if matches!(operation,Operation::Shutdown){let _=reply.send(Ok(Settings::default()));break;}let result=state.execute(&context,operation);let _=reply.send(result);drop(activity);}
  });
  if worker.is_err(){let mut state=self.context.gate.state.lock().map_err(|_|UNAVAILABLE)?;state.2=false;self.context.gate.changed.notify_all();return Err(UNAVAILABLE.into());}
  *sender=Some(tx.clone());Ok(tx)
 }
 async fn call(&self,operation:Operation)->Result<Settings,String>{let activity=self.context.gate.reserve()?;let sender=self.sender()?;let(tx,rx)=tokio::sync::oneshot::channel();sender.send(Message{operation,reply:tx,activity:Some(activity)}).map_err(|_|UNAVAILABLE)?;rx.await.map_err(|_|UNAVAILABLE.to_owned())?}
 pub(crate) async fn settings(&self)->Result<Settings,String>{self.call(Operation::Status).await}
 pub(crate) async fn apply_enabled_from_config(&self,enabled:bool)->Result<Settings,String>{self.call(Operation::Enabled(enabled)).await}
 pub(crate) async fn prepare_account(&self,settings:Settings)->Result<PreparedAccount,String>{
  let mut account=if settings.plex_token.is_empty()&&settings.plex_url.is_empty(){None}else{Some(store::Account::from_settings(&settings)?)};
  if settings.enabled {if let Some(account)=account.as_mut(){let client=Client::new(&account.settings(true))?;let server=self.read(async{let connection=client.manager.resolve().await.map_err(|e|e.public_message())?;client.manager.validate_source_connection(&connection).map_err(|e|e.public_message())?;Ok(connection.server_id.clone())}).await?;account.server_id=Some(server);}}
  Ok(PreparedAccount(account))
 }
 pub(crate) async fn configure(&self,enabled:bool,account:Option<PreparedAccount>)->Result<Settings,String>{self.call(Operation::Configure(enabled,account)).await}
 pub(crate) async fn commit_account(&self,account:PreparedAccount)->Result<Settings,String>{self.call(Operation::Commit(account)).await}
 pub(super) fn registry(&self)->Arc<Registry>{self.context.registry.clone()}
 pub(super) async fn read<T>(&self,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{let _activity=self.context.gate.reserve()?;self.context.read(work).await}
 // Original UI can admit eight covers at once. Queue bounded artwork work
 // separately, leaving all catalog/playback/control capacity available.
 pub(super) async fn artwork_read<T>(&self,binding:&Arc<Binding>,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{
  let _activity=self.context.gate.reserve_image()?;self.context.check()?;
  let mut stop=self.context.stop.subscribe();let mut source=binding.stop.subscribe();if !binding.active(){return Err(CHANGED.into());}
  let result=tokio::select!{biased;
   _=stop.wait_for(|closed|*closed)=>Err(CHANGED.into()),
   _=source.wait_for(|closed|*closed)=>Err(CHANGED.into()),
   result=tokio::time::timeout(Duration::from_secs(60),async{
    let _slot=self.context.artwork.clone().acquire_owned().await.map_err(|_|CHANGED)?;
    self.context.check()?;if !binding.active(){return Err(CHANGED.into());}work.await
   })=>result.map_err(|_|UNAVAILABLE)?,
  };self.context.check()?;result
 }
 pub(super) async fn ab_read<T>(&self,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{let _activity=self.context.gate.reserve()?;self.context.read_budget(Duration::from_secs(60),work).await}
 pub(super) async fn catalog_read<T>(&self,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{let _activity=self.context.gate.reserve()?;self.context.read_budget(Duration::from_secs(120),work).await}
 pub(crate) fn begin_shutdown(&self){self.context.gate.close();self.context.stop.send_replace(true);self.context.network.close();self.context.artwork.close();let sender=self.sender.lock().unwrap_or_else(|e|e.into_inner()).take();if let Some(sender)=sender{let(reply,_)=tokio::sync::oneshot::channel();let _=sender.send(Message{operation:Operation::Shutdown,reply,activity:None});}}
 pub(crate) fn shutdown(&self)->Result<(),String>{self.begin_shutdown();let end=Instant::now()+Duration::from_secs(3);let mut state=self.context.gate.state.lock().map_err(|_|UNAVAILABLE)?;while state.1!=0||state.2{let remaining=end.saturating_duration_since(Instant::now());if remaining.is_zero(){return Err(UNAVAILABLE.into());}state=self.context.gate.changed.wait_timeout(state,remaining).map_err(|_|UNAVAILABLE)?.0;}Ok(())}
}

#[cfg(test)]
#[path="host_tests.rs"]
mod tests;

#[cfg(test)]
impl Controller {
 pub(crate) fn smart_test_host(path:PathBuf,settings:Settings)->Self {
  struct Memory(Mutex<Option<store::Account>>);
  impl Vault for Memory {
   fn load(&self,_:&Path)->Result<Option<store::Account>,String>{Ok(self.0.lock().unwrap().clone())}
   fn save(&self,_:&Path,value:Option<store::Account>)->Result<(),String>{*self.0.lock().unwrap()=value;Ok(())}
  }
  fn own()->Result<(),String>{Ok(())}
  let account=store::Account::from_settings(&settings).unwrap();
  Self::with_parts(path,Arc::new(Memory(Mutex::new(Some(account)))),own).unwrap()
 }
}
