use super::*;
#[derive(Default)]struct MemoryVault{account:Mutex<Option<store::Account>>,fail_load:std::sync::atomic::AtomicBool,fail_save:std::sync::atomic::AtomicBool,blocked:Mutex<bool>,changed:Condvar,entered:std::sync::atomic::AtomicBool,writes:std::sync::atomic::AtomicUsize}
impl Vault for MemoryVault {
 fn load(&self,_:&Path)->Result<Option<store::Account>,String>{if self.fail_load.load(std::sync::atomic::Ordering::Acquire){Err(STORAGE.into())}else{Ok(self.account.lock().unwrap().clone())}}
 fn save(&self,_:&Path,account:Option<store::Account>)->Result<(),String>{self.entered.store(true,std::sync::atomic::Ordering::Release);let blocked=self.blocked.lock().unwrap();let _blocked=self.changed.wait_while(blocked,|b|*b).unwrap();if self.fail_save.load(std::sync::atomic::Ordering::Acquire){return Err(STORAGE.into());}*self.account.lock().unwrap()=account;self.writes.fetch_add(1,std::sync::atomic::Ordering::AcqRel);Ok(())}
}
fn valid_owner()->Result<(),String>{Ok(())}
fn account(id:&str)->store::Account{store::Account{server_url:"http://fixture.invalid".into(),account_token:format!("FIXTURE_{id}"),server_id:Some(id.into()),server_name:Some("Fixture".into())}}
fn host(vault:Arc<MemoryVault>)->Controller{Controller::with_parts(std::env::temp_dir().join("sonante-plex-host-fixture-unused.json"),vault,valid_owner).unwrap()}

#[tokio::test]
async fn pcm_signal_observes_retirement_without_retaining_account_or_listener(){
 let host=host(Arc::new(MemoryVault::default()));
 host.configure(true,Some(PreparedAccount(Some(account("health-fixture"))))).await.unwrap();
 let binding=host.registry().selected().unwrap();let weak=Arc::downgrade(&binding);
 let watch=crate::windows_mpd::liveness::Watch::new(vec![crate::windows_mpd::liveness::Signal::remote(binding.health_signal())]).unwrap();
 assert!(watch.healthy());assert!(binding.port.lock().unwrap().is_none());
 host.shutdown().unwrap();assert!(!watch.healthy());drop(binding);assert!(weak.upgrade().is_none());
}
async fn until(condition:impl Fn()->bool){let end=Instant::now()+Duration::from_secs(2);while !condition(){assert!(Instant::now()<end);tokio::time::sleep(Duration::from_millis(2)).await;}}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn actual_host_keeps_credentials_separate_from_enabled_and_restore_never_makes_http_requests(){
 let vault=Arc::new(MemoryVault::default());let host=host(vault.clone());assert!(host.settings().await.unwrap().plex_token.is_empty());host.commit_account(PreparedAccount(Some(account("server-one")))).await.unwrap();assert!(host.registry().selected().is_err());
 host.apply_enabled_from_config(true).await.unwrap();let binding=host.registry().selected().unwrap();assert!(binding.active());assert_eq!(binding.settings.plex_server_id.as_deref(),Some("server-one"));host.apply_enabled_from_config(false).await.unwrap();assert!(!binding.active());assert_eq!(host.settings().await.unwrap().plex_token,"FIXTURE_server-one");host.shutdown().unwrap();
 let reopened=super::tests::host(vault);let settings=reopened.settings().await.unwrap();assert!(!settings.enabled);assert_eq!(settings.plex_token,"FIXTURE_server-one");assert!(reopened.registry().selected().is_err());reopened.shutdown().unwrap();
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn corrupt_vault_and_failed_atomic_save_preserve_current_binding_and_account(){
 let vault=Arc::new(MemoryVault::default());let host=host(vault.clone());host.commit_account(PreparedAccount(Some(account("server-one")))).await.unwrap();host.apply_enabled_from_config(true).await.unwrap();let binding=host.registry().selected().unwrap();vault.fail_save.store(true,std::sync::atomic::Ordering::Release);
 assert_eq!(host.commit_account(PreparedAccount(Some(account("server-two")))).await.err().as_deref(),Some(STORAGE));assert!(binding.active());assert!(Arc::ptr_eq(&binding,&host.registry().selected().unwrap()));assert_eq!(host.settings().await.unwrap().plex_server_id.as_deref(),Some("server-one"));assert_eq!(vault.writes.load(std::sync::atomic::Ordering::Acquire),1);host.shutdown().unwrap();
 let bad=Arc::new(MemoryVault::default());bad.fail_load.store(true,std::sync::atomic::Ordering::Release);let host=super::tests::host(bad.clone());assert!(host.commit_account(PreparedAccount(Some(account("server-one")))).await.is_err());assert_eq!(bad.writes.load(std::sync::atomic::Ordering::Acquire),0);host.shutdown().unwrap();
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn cancelling_the_awaiter_never_releases_physical_write_admission_or_reorders_later_commands(){
 let vault=Arc::new(MemoryVault::default());*vault.blocked.lock().unwrap()=true;let host=host(vault.clone());let h=host.clone();let task=tokio::spawn(async move{h.commit_account(PreparedAccount(Some(account("server-one")))).await});until(||vault.entered.load(std::sync::atomic::Ordering::Acquire)).await;task.abort();let _=task.await;
 assert_eq!(host.context.gate.state.lock().unwrap().1,1);let h=host.clone();let next=tokio::spawn(async move{h.commit_account(PreparedAccount(Some(account("server-two")))).await});until(||host.context.gate.state.lock().unwrap().1==2).await;assert!(!next.is_finished());
 *vault.blocked.lock().unwrap()=false;vault.changed.notify_all();next.await.unwrap().unwrap();assert_eq!(host.settings().await.unwrap().plex_server_id.as_deref(),Some("server-two"));assert_eq!(vault.writes.load(std::sync::atomic::Ordering::Acquire),2);host.shutdown().unwrap();
}
#[tokio::test(flavor="current_thread")]
async fn terminal_registry_effect_wait_runs_on_owned_os_worker_without_blocking_the_tokio_executor(){
 let host=host(Arc::new(MemoryVault::default()));host.commit_account(PreparedAccount(Some(account("server-one")))).await.unwrap();host.apply_enabled_from_config(true).await.unwrap();let registry=host.registry();let binding=registry.selected().unwrap();let (held,ready)=mpsc::channel();let(release,wait)=mpsc::channel();let thread=std::thread::spawn(move||registry.with_current(&binding,||{held.send(()).unwrap();wait.recv_timeout(Duration::from_secs(2)).unwrap();Ok(())}));ready.recv_timeout(Duration::from_secs(1)).unwrap();host.begin_shutdown();
 tokio::time::timeout(Duration::from_millis(100),tokio::time::sleep(Duration::from_millis(15))).await.unwrap();assert!(host.context.gate.state.lock().unwrap().2);release.send(()).unwrap();thread.join().unwrap().unwrap();host.shutdown().unwrap();assert!(host.registry().selected().is_err());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn real_plex_metadata_proxy_range_head_lease_restore_and_disconnect_use_only_fixture_http(){
 use tokio::io::{AsyncReadExt,AsyncWriteExt};
 use crate::windows_mpd::{queue_v3::Source,library::Track};
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let url=format!("http://{address}");let requests=Arc::new(Mutex::new(Vec::<String>::new()));let recorded=requests.clone();let behavior=Arc::new(std::sync::atomic::AtomicU8::new(0));let mode=behavior.clone();let(stop,mut stopped)=tokio::sync::watch::channel(false);
 let server=tokio::spawn(async move {loop{let accepted=tokio::select!{biased;_=stopped.wait_for(|v|*v)=>break,value=listener.accept()=>value};let(mut socket,_)=accepted.unwrap();let recorded=recorded.clone();let mode=mode.clone();tokio::spawn(async move{let mut data=Vec::new();loop{let mut chunk=[0u8;1024];let n=socket.read(&mut chunk).await.unwrap();if n==0{return;}data.extend_from_slice(&chunk[..n]);if data.windows(4).any(|v|v==b"\r\n\r\n"){break;}assert!(data.len()<8192);}let request=String::from_utf8(data).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let path=target.split('?').next().unwrap();recorded.lock().unwrap().push(path.to_owned());
 let (status,headers,body)=match path {
  "/identity"=>("200 OK",String::from("Content-Type: application/json\r\n"),serde_json::json!({"MediaContainer":{"machineIdentifier":"server-one"}}).to_string().into_bytes()),
  "/library/sections"=>("200 OK",String::from("Content-Type: application/json\r\n"),b"{\"MediaContainer\":{}}".to_vec()),
  "/library/metadata/song-one"=>{let mut part=serde_json::json!({"key":"/library/parts/1/file.flac","exists":true,"Stream":[{"streamType":2,"codec":"flac","samplingRate":96000,"bitDepth":24}]});let mode=mode.load(std::sync::atomic::Ordering::Acquire);if mode==1{part.as_object_mut().unwrap().remove("exists");}if mode==2{part["exists"]=serde_json::json!(false);}let parts=if mode==4{vec![part.clone(),part]}else{vec![part]};("200 OK",String::from("Content-Type: application/json\r\n"),serde_json::json!({"MediaContainer":{"Metadata":[{"type":"track","duration":123456,"parentRatingKey":"album-one","grandparentRatingKey":"artist-one","parentTitle":"Fixture Album","grandparentTitle":"Fixture Artist","parentThumb":if mode==5{"http://foreign.invalid/image"}else if mode==6{"/library/metadata/FIXTURE_server-one/thumb"}else if mode==7{"/library/metadata/%46IXTURE_server-one/thumb"}else{"/library/metadata/album-one/thumb/1"},"ratingKey":if mode==3{"wrong-song"}else{"song-one"},"Media":[{"audioCodec":"flac","Part":parts}]}]}}).to_string().into_bytes())},
  "/library/parts/1/file.flac"=>if request.to_ascii_lowercase().contains("range: bytes=3-5"){("206 Partial Content",String::from("Content-Type: audio/flac\r\nContent-Range: bytes 3-5/12\r\nAccept-Ranges: bytes\r\n"),b"345".to_vec())}else{("200 OK",String::from("Content-Type: audio/flac\r\nAccept-Ranges: bytes\r\n"),b"0123456789ab".to_vec())},
  _=>("404 Not Found",String::new(),Vec::new()),
 };let header=format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",body.len());socket.write_all(header.as_bytes()).await.unwrap();socket.write_all(&body).await.unwrap();});}});
 let vault=Arc::new(MemoryVault::default());let host=host(vault);let mut account=account("server-one");account.server_url=url;host.commit_account(PreparedAccount(Some(account))).await.unwrap();host.apply_enabled_from_config(true).await.unwrap();
 let registry=host.registry();let settings=host.settings().await.unwrap();let mut binding=Binding::new(settings.clone()).unwrap();let fresh=Arc::get_mut(&mut binding).unwrap();let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().unwrap();let(tx,_)=tokio::sync::watch::channel(false);fresh.client=Client{manager:Arc::new(super::super::core::PlexConnectionManager::fixture(&settings,http.clone())),network:Arc::new(tokio::sync::Semaphore::new(4)),artwork:Arc::new(tokio::sync::Semaphore::new(2)),stop:tx};fresh.stop=fresh.client.stop.clone();fresh.stream_http=http.clone();registry.replace(Some(binding.clone()),||Ok(())).unwrap();assert!(binding.port.lock().unwrap().is_none());
 let source=Source::Plex{server_id:"server-one".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:"song-one".into()};let row=Track{id:"song-one".into(),title:"Fixture".into(),artist:"Fixture".into(),album:"Fixture".into(),disc:1,number:1,file_name:String::new(),catalog_album_id:None,path:Default::default(),media_root:None,radio:None,upnp:None,remote_source:Some(source.clone()),remote_connection_id:None,plex_image:None,remote_lease:None};
 let locator=crate::audio::MediaLocator::Plex{server_id:"server-one".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:Some("song-one".into()),file_path:None};assert_eq!(host.available(locator.clone()).await.unwrap(),Some(true));assert!(binding.port.lock().unwrap().is_none(),"availability does not open a media listener");
 let facts=host.source_facts(locator.clone(),async{true}).await.unwrap();let info=serde_json::to_value(facts).unwrap();assert_eq!(info["codec"],"FLAC");assert_eq!(info["sample_rate"],96000);assert_eq!(info["bit_depth"],24);for key in ["path","token","output","bitPerfect"]{assert!(info.get(key).is_none());}
 assert!(host.source_facts(locator.clone(),async{false}).await.is_none());
 let metadata_before=requests.lock().unwrap().iter().filter(|p|p.as_str()=="/library/metadata/song-one").count();
 let prepared=host.prepare_tracks(vec![row.clone(),row.clone()]).await.unwrap();assert_eq!(prepared.len(),2);assert!(prepared.iter().all(|t|t.remote_lease.as_ref().unwrap().presentation_duration()==Some(123.456)));
 assert_eq!(requests.lock().unwrap().iter().filter(|p|p.as_str()=="/library/metadata/song-one").count()-metadata_before,2,"artwork reuses the admitted metadata proof, no extra request");
 assert_eq!(prepared[0].plex_image,Some(crate::plex::PlexImageRef{server_id:"server-one".into(),path:"/library/metadata/album-one/thumb/1".into()}));let lease=prepared[0].remote_lease.as_ref().unwrap().clone();let uri=lease.uri().to_owned();assert_ne!(lease.connection_id(),binding.secret);assert!(!uri.contains("FIXTURE_server-one"));assert!(!uri.contains("library/parts"));
 let response=http.get(&uri).header(reqwest::header::RANGE,"bytes=3-5").send().await.unwrap();assert_eq!(response.status(),206);assert_eq!(response.bytes().await.unwrap().as_ref(),b"345");let response=http.head(&uri).send().await.unwrap();assert_eq!(response.headers().get(reqwest::header::CONTENT_LENGTH).unwrap(),"12");assert!(response.bytes().await.unwrap().is_empty());

 let before=requests.lock().unwrap().iter().filter(|p|p.as_str()=="/library/parts/1/file.flac").count();
 for invalid in [format!("{uri}x"),uri.replace(&binding.secret,&"0".repeat(48)),format!("{}/{}",uri.rsplit_once('/').unwrap().0,"0".repeat(48))]{assert_eq!(http.get(invalid).send().await.unwrap().status(),404);}
 for range in ["bytes=1-2,4-5","bytes=-","bytes=not-valid"]{assert_eq!(http.get(&uri).header(reqwest::header::RANGE,range).send().await.unwrap().status(),404);}
 assert_eq!(http.get(&uri).header(reqwest::header::RANGE,"bytes=1-2").header(reqwest::header::RANGE,"bytes=3-4").send().await.unwrap().status(),404);
 let transient=host.prepare_tracks(vec![row.clone()]).await.unwrap();let released=transient[0].remote_lease.as_ref().unwrap().uri().to_owned();drop(transient);assert_eq!(http.get(released).send().await.unwrap().status(),404);
 assert_eq!(requests.lock().unwrap().iter().filter(|p|p.as_str()=="/library/parts/1/file.flac").count(),before,"invalid and released capabilities must not reach PMS");
 for mode in [5,6,7]{behavior.store(mode,std::sync::atomic::Ordering::Release);let without_art=host.prepare_tracks(vec![row.clone()]).await.unwrap();assert!(without_art[0].plex_image.is_none(),"invalid or secret-bearing artwork never changes playable reference");}behavior.store(0,std::sync::atomic::Ordering::Release);
 let saved=crate::windows_mpd::TestSavedQueue::capture(Path::new(""),&prepared,1,7.0,12.0,35,None);let encoded=serde_json::to_string(&saved).unwrap();for denied in [&uri,"FIXTURE_server-one",binding.secret.as_str(),binding.generation.as_str()]{assert!(!encoded.contains(denied));}let mut id=[0;12];getrandom::fill(&mut id).unwrap();let folder=std::env::temp_dir().join(format!("sonante-plex-artwork-{id:x?}"));std::fs::create_dir(&folder).unwrap();let file=folder.join("queue.json");let mut journal=crate::windows_mpd::TestJournal::open_chain(file.clone(),Vec::new());journal.replace_logical(Some(saved.clone())).unwrap();let bytes=std::fs::read_to_string(&file).unwrap();assert!(bytes.contains("album-one/thumb"));assert!(!bytes.contains("FIXTURE_server-one"));let reopened=crate::windows_mpd::TestJournal::open_chain(file,Vec::new());let logical=reopened.saved.unwrap().selection();assert!(logical.tracks.iter().all(|t|t.plex_image==prepared[0].plex_image&&t.remote_lease.is_none()));std::fs::remove_dir_all(folder).unwrap();
 let restored=host.prepare_tracks(logical.tracks).await.unwrap();assert_eq!(restored[0].plex_image,prepared[0].plex_image);assert_eq!(restored.len(),2);assert!(restored.iter().all(|t|t.remote_lease.as_ref().unwrap().presentation_duration()==Some(123.456)));assert!(restored.iter().all(|t|t.remote_lease.as_ref().unwrap().healthy()));
 behavior.store(1,std::sync::atomic::Ordering::Release);assert_eq!(host.available(locator.clone()).await.unwrap(),None);behavior.store(2,std::sync::atomic::Ordering::Release);assert_eq!(host.available(locator.clone()).await.unwrap(),Some(false));assert!(host.prepare_tracks(vec![row.clone()]).await.is_err());behavior.store(3,std::sync::atomic::Ordering::Release);assert!(host.available(locator.clone()).await.is_err());behavior.store(4,std::sync::atomic::Ordering::Release);assert!(host.available(locator).await.is_err());
 let mut effects=0;lease.command(&mut||{effects+=1;Ok(Vec::new())}).unwrap();assert_eq!(effects,1);
 behavior.store(0,std::sync::atomic::Ordering::Release);
 let navigation_locator=crate::audio::MediaLocator::Plex{server_id:"server-one".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:Some("song-one".into()),file_path:None};
 let navigation=host.history_navigation(navigation_locator.clone(),async{Ok(true)}).await.unwrap();assert_eq!(navigation.album.rating_key,"album-one");
 assert_eq!(host.history_navigation(navigation_locator.clone(),async{Ok(false)}).await.err().as_deref(),Some("HISTORY_UNAVAILABLE"));
 let changed=host.clone();assert_eq!(host.history_navigation(navigation_locator,async move{changed.apply_enabled_from_config(false).await?;Ok(true)}).await.err().as_deref(),Some("PLEX_CONTEXT_CHANGED"));
 // The retired lease must not issue a media command after navigation's final fence.
 assert!(lease.command(&mut||{effects+=1;Ok(Vec::new())}).is_err());assert_eq!(effects,1);
 host.shutdown().unwrap();stop.send_replace(true);server.await.unwrap();assert!(requests.lock().unwrap().iter().any(|path|path=="/library/parts/1/file.flac"));
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn account_and_source_flag_publish_once_and_failed_store_keeps_both_previous_authorities(){
 let vault=Arc::new(MemoryVault::default());let host=host(vault.clone());host.configure(true,Some(PreparedAccount(Some(account("server-one"))))).await.unwrap();let old=host.registry().selected().unwrap();vault.fail_save.store(true,std::sync::atomic::Ordering::Release);
 assert!(host.configure(false,Some(PreparedAccount(Some(account("server-two"))))).await.is_err());assert!(old.active());let settings=host.settings().await.unwrap();assert!(settings.enabled);assert_eq!(settings.plex_server_id.as_deref(),Some("server-one"));
 vault.fail_save.store(false,std::sync::atomic::Ordering::Release);let settings=host.configure(false,Some(PreparedAccount(Some(account("server-two"))))).await.unwrap();assert!(!settings.enabled);assert!(!old.active());assert_eq!(settings.plex_server_id.as_deref(),Some("server-two"));assert!(host.registry().selected().is_err());host.apply_enabled_from_config(true).await.unwrap();assert_eq!(host.registry().selected().unwrap().settings.plex_server_id.as_deref(),Some("server-two"));host.shutdown().unwrap();
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn public_account_projection_has_no_token_and_saved_clear_edits_remain_backend_owned(){
 let vault=Arc::new(MemoryVault::default());let host=host(vault.clone());
 let saved=||serde_json::from_value::<super::super::AccountDraft>(serde_json::json!({"url":"http://fixture.invalid","server_id":"server-one","server_name":"Fixture","credential":{"kind":"saved"}})).unwrap();
 assert!(host.prepare_draft(false,saved()).await.is_err());
 host.commit_account(PreparedAccount(Some(account("server-one")))).await.unwrap();let json=serde_json::to_value(host.public_settings().await.unwrap()).unwrap();assert_eq!(json["saved"],true);assert_eq!(json["server_id"],"server-one");let encoded=json.to_string();assert!(!encoded.contains("FIXTURE_server-one"));assert!(!encoded.contains("token"));
 let prepared=host.prepare_draft(false,saved()).await.unwrap();host.configure(false,Some(prepared)).await.unwrap();assert_eq!(vault.account.lock().unwrap().as_ref().unwrap().account_token,"FIXTURE_server-one");
 let clear=serde_json::from_value(serde_json::json!({"url":"","server_id":null,"server_name":null,"credential":{"kind":"clear"}})).unwrap();let prepared=host.prepare_draft(false,clear).await.unwrap();host.configure(false,Some(prepared)).await.unwrap();assert!(vault.account.lock().unwrap().is_none());assert!(!host.public_settings().await.unwrap().saved);host.shutdown().unwrap();
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn original_eight_image_burst_completes_without_starving_the_catalog(){
 use tokio::io::{AsyncReadExt,AsyncWriteExt};use std::sync::atomic::{AtomicUsize,Ordering};
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let settings=Settings{enabled:true,plex_url:format!("http://{address}"),plex_token:"FIXTURE_IMAGE_TOKEN".into(),plex_server_id:Some("server-one".into()),plex_server_name:Some("Fixture".into())};
 let active=Arc::new(AtomicUsize::new(0));let maximum=Arc::new(AtomicUsize::new(0));let count=Arc::new(AtomicUsize::new(0));let a=active.clone();let m=maximum.clone();let n=count.clone();let(stop,mut stopping)=tokio::sync::watch::channel(false);
 let server=tokio::spawn(async move{loop{let socket=tokio::select!{biased;_=stopping.wait_for(|v|*v)=>break,value=listener.accept()=>value};let(mut socket,_)=socket.unwrap();let active=a.clone();let maximum=m.clone();let count=n.clone();tokio::spawn(async move{let mut data=Vec::new();loop{let mut bytes=[0u8;1024];let n=socket.read(&mut bytes).await.unwrap();if n==0{return;}data.extend_from_slice(&bytes[..n]);if data.windows(4).any(|v|v==b"\r\n\r\n"){break;}assert!(data.len()<=8192);}let request=String::from_utf8(data).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let path=target.split('?').next().unwrap();let(body,kind)=if path=="/identity"{(br#"{"MediaContainer":{"machineIdentifier":"server-one"}}"#.to_vec(),"application/json")}else if path=="/library/sections"{(br#"{"MediaContainer":{"Directory":[]}}"#.to_vec(),"application/json")}else if path.starts_with("/library/metadata/"){
  let current=active.fetch_add(1,Ordering::AcqRel)+1;maximum.fetch_max(current,Ordering::AcqRel);count.fetch_add(1,Ordering::AcqRel);tokio::time::sleep(Duration::from_millis(80)).await;active.fetch_sub(1,Ordering::AcqRel);(b"FIXTURE_IMAGE_BYTES".to_vec(),"image/jpeg")
 }else{panic!("unexpected fixture path")};let header=format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());let _=socket.write_all(header.as_bytes()).await;let _=socket.write_all(&body).await;});}});
 let vault=Arc::new(MemoryVault::default());let host=host(vault);host.commit_account(PreparedAccount(Some(store::Account::from_settings(&settings).unwrap()))).await.unwrap();host.apply_enabled_from_config(true).await.unwrap();let registry=host.registry();let mut binding=Binding::new(settings.clone()).unwrap();let fresh=Arc::get_mut(&mut binding).unwrap();let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().unwrap();let(tx,_)=tokio::sync::watch::channel(false);fresh.client=Client{manager:Arc::new(super::super::core::PlexConnectionManager::fixture(&settings,http)),network:Arc::new(tokio::sync::Semaphore::new(4)),artwork:Arc::new(tokio::sync::Semaphore::new(2)),stop:tx};fresh.stop=fresh.client.stop.clone();registry.replace(Some(binding),||Ok(())).unwrap();
 let mut jobs=tokio::task::JoinSet::new();for i in 0..8{let host=host.clone();jobs.spawn(async move{host.image(crate::plex::PlexImageRef{server_id:"server-one".into(),path:format!("/library/metadata/{i}/thumb")}).await});}
 until(||active.load(Ordering::Acquire)>=2).await;
 let catalog=host.libraries().await;let mut failures=Vec::new();let mut success=0;while let Some(result)=jobs.join_next().await{match result.unwrap(){Ok(bytes)=>{assert_eq!(bytes,b"FIXTURE_IMAGE_BYTES");success+=1;},Err(error)=>failures.push(error)}}
 host.shutdown().unwrap();stop.send_replace(true);server.await.unwrap();
 assert!(catalog.is_ok(),"original artwork burst must not consume catalog capacity: {catalog:?}");assert_eq!(success,8,"failed original image requests: {failures:?}");assert_eq!(count.load(Ordering::Acquire),8);assert!(maximum.load(Ordering::Acquire)<=2,"image I/O capacity is independently bounded");
}

#[test]
fn artwork_backlog_preserves_control_admission_and_all_work_is_drained(){
 let gate=Arc::new(Gate::default());let mut images=Vec::new();for _ in 0..12{images.push(gate.reserve_image().unwrap());}assert!(gate.reserve_image().is_err());
 let mut control=Vec::new();for _ in 0..16{control.push(gate.reserve().unwrap());}assert!(gate.reserve().is_err());{let s=gate.state.lock().unwrap();assert_eq!((s.1,s.3),(28,12));}
 images.pop();assert!(gate.reserve_image().is_ok());assert!(gate.reserve().is_err());gate.close();assert!(gate.reserve_image().is_err());assert!(gate.reserve().is_err());drop(images);drop(control);let s=gate.state.lock().unwrap();assert_eq!((s.1,s.3),(0,0));
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn queued_artwork_is_cancelled_on_disconnect_without_waiting_for_an_image_permit(){
 let host=host(Arc::new(MemoryVault::default()));host.commit_account(PreparedAccount(Some(account("server-one")))).await.unwrap();host.apply_enabled_from_config(true).await.unwrap();
 let held=host.context.artwork.clone().acquire_many_owned(2).await.unwrap();let h=host.clone();let job=tokio::spawn(async move{h.image(crate::plex::PlexImageRef{server_id:"server-one".into(),path:"/library/metadata/one/thumb".into()}).await});
 until(||host.context.gate.state.lock().unwrap().3==1).await;host.apply_enabled_from_config(false).await.unwrap();let result=tokio::time::timeout(Duration::from_millis(200),job).await.unwrap().unwrap();assert_eq!(result.err().as_deref(),Some(CHANGED));assert_eq!(host.context.gate.state.lock().unwrap().3,0);drop(held);host.shutdown().unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn queued_presentation_artwork_matches_linux_server_sanitization_without_network_or_media_lease(){
 use crate::windows_mpd::queue_v3::Source;
 let host=host(Arc::new(MemoryVault::default()));host.commit_account(PreparedAccount(Some(account("server-one")))).await.unwrap();host.apply_enabled_from_config(true).await.unwrap();
 let source=Source::Plex{server_id:"server-one".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:"song-one".into()};let image=|server:&str,path:&str|Some(crate::plex::PlexImageRef{server_id:server.into(),path:path.into()});
 assert!(host.presentation_image(&source,image("server-one","/library/metadata/album-one/thumb/1")).is_some());
 for(server,path)in [("other","/library/metadata/album-one/thumb"),("server-one","/library/metadata/FIXTURE_server-one/thumb"),("server-one","/library/metadata/%46IXTURE_server-one/thumb"),("server-one","/library/metadata/one/thumb?X-Plex-Token=SECRET"),("server-one","http://fixture.invalid/image")]{assert!(host.presentation_image(&source,image(server,path)).is_none());}
 let binding=host.registry().selected().unwrap();assert!(binding.port.lock().unwrap().is_none());host.apply_enabled_from_config(false).await.unwrap();assert!(host.presentation_image(&source,image("server-one","/library/metadata/album-one/thumb")).is_none());host.shutdown().unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn editions_profile_is_fenced_to_exact_server_complete_inventory_and_retired_binding(){
 use tokio::io::{AsyncReadExt,AsyncWriteExt};use std::sync::atomic::{AtomicBool,Ordering};
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let requests=Arc::new(Mutex::new(Vec::new()));let seen=requests.clone();
 let incomplete=Arc::new(AtomicBool::new(false));let bad=incomplete.clone();let block=Arc::new(AtomicBool::new(false));let blocked=block.clone();let entered=Arc::new(tokio::sync::Notify::new());let admitted=entered.clone();let released=Arc::new(tokio::sync::Notify::new());let release=released.clone();let(stop,mut stopped)=tokio::sync::watch::channel(false);
 let peer=tokio::spawn(async move{loop{let accepted=tokio::select!{_=stopped.changed()=>break,r=listener.accept()=>r.unwrap()};let(mut socket,_)=accepted;let mut bytes=Vec::new();while !bytes.ends_with(b"\r\n\r\n"){bytes.push(socket.read_u8().await.unwrap());assert!(bytes.len()<8192);}let request=String::from_utf8(bytes).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let url=reqwest::Url::parse(&format!("http://{address}{target}")).unwrap();seen.lock().unwrap().push(url.path().to_owned());
 let body=match url.path(){
  "/identity"=>serde_json::json!({"MediaContainer":{"machineIdentifier":"server-one"}}),
  "/library/sections"=>serde_json::json!({"MediaContainer":{"Directory":[{"key":"7","type":"artist","title":"Own Library"}]}}),
  "/library/metadata/album-one/children"=>{assert!(url.query_pairs().any(|(k,v)|k=="X-Plex-Container-Size"&&v=="500"));if blocked.load(Ordering::Acquire){admitted.notify_one();tokio::time::timeout(Duration::from_secs(3),release.notified()).await.unwrap();}serde_json::json!({"MediaContainer":{"librarySectionID":7,"size":1,"totalSize":if bad.load(Ordering::Acquire){2}else{1},"offset":0,"Metadata":[{"type":"track","ratingKey":"t1","parentRatingKey":"album-one","title":"One","Media":[{"Part":[{"file":"/private/Own Album (Deluxe)/one.flac","size":123}]}]}]}})},
  other=>panic!("Unexpected fixture path {other}"),
 }.to_string();let _=socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
 }});
 let h=host(Arc::new(MemoryVault::default()));let mut a=account("server-one");a.server_url=format!("http://{address}");h.configure(true,Some(PreparedAccount(Some(a)))).await.unwrap();let settings=h.settings().await.unwrap();
 let mut binding=Binding::new(settings.clone()).unwrap();let fresh=Arc::get_mut(&mut binding).unwrap();let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().unwrap();let(tx,_)=tokio::sync::watch::channel(false);fresh.client=Client{manager:Arc::new(super::super::core::PlexConnectionManager::fixture(&settings,http.clone())),network:Arc::new(tokio::sync::Semaphore::new(4)),artwork:Arc::new(tokio::sync::Semaphore::new(2)),stop:tx};fresh.stop=fresh.client.stop.clone();fresh.stream_http=http;
 h.registry().replace(Some(binding.clone()),||Ok(())).unwrap();assert!(h.edition_profile("foreign".into(),"7".into(),"album-one".into()).await.is_err());assert!(requests.lock().unwrap().is_empty());
 let p=h.edition_profile("server-one".into(),"7".into(),"album-one".into()).await.unwrap().unwrap();assert_eq!(p.sizes,Some(vec![123]));assert_eq!(p.edition_title.as_deref(),Some("Own Album (Deluxe)"));let json=serde_json::to_string(&p).unwrap();assert!(!json.contains("/private")&&!json.contains("FIXTURE_"));assert!(binding.port.lock().unwrap().is_none());
 incomplete.store(true,Ordering::Release);assert!(h.edition_profile("server-one".into(),"7".into(),"album-one".into()).await.unwrap().is_none());incomplete.store(false,Ordering::Release);
 block.store(true,Ordering::Release);let worker=h.clone();let reading=tokio::spawn(async move{worker.edition_profile("server-one".into(),"7".into(),"album-one".into()).await});tokio::time::timeout(Duration::from_secs(3),entered.notified()).await.unwrap();h.apply_enabled_from_config(false).await.unwrap();released.notify_one();assert!(reading.await.unwrap().is_err());assert!(!binding.active());assert!(binding.port.lock().unwrap().is_none());h.shutdown().unwrap();stop.send_replace(true);peer.await.unwrap();
}

#[path="ab_tests.rs"]
mod ab_tests;
