use super::*;
use std::{path::PathBuf,sync::{Arc,Mutex}};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
struct Environment{dir:PathBuf,library:Library,store:smart::Controller,favorites:Favorites,nav:crate::windows_navidrome::Controller,plex:crate::windows_plex::Controller,requests:Arc<Mutex<Vec<String>>>,stop:tokio::sync::watch::Sender<bool>,peer:tokio::task::JoinHandle<()>}
impl Environment{
 async fn new()->Self{
  let dir=std::env::temp_dir().join(format!("sonante-smart-remote-{}",crate::windows_navidrome::api::random().unwrap()));std::fs::create_dir(&dir).unwrap();
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let url=format!("http://{address}/");
  let requests=Arc::new(Mutex::new(Vec::new()));let seen=requests.clone();let(stop,mut receive)=tokio::sync::watch::channel(false);
  let peer=tokio::spawn(async move{let mut children=tokio::task::JoinSet::new();loop{
   let accepted=tokio::select!{_=receive.changed()=>break,r=listener.accept()=>r};let(mut socket,_)=accepted.unwrap();let seen=seen.clone();children.spawn(async move{
    let mut request=Vec::new();while !request.ends_with(b"\r\n\r\n"){request.push(socket.read_u8().await.unwrap());assert!(request.len()<8192);}
    let request=String::from_utf8(request).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let url=reqwest::Url::parse(&format!("http://{address}{target}")).unwrap();seen.lock().unwrap().push(url.path().into());
    let album=json!({"id":"nav-album","name":"Nav Album","artist":"Artist","genre":"Jazz","created":"2026-10-31T00:00:00Z","year":1999,"song":[{"id":"nav-song","title":"Track"}]});
    let body=match url.path(){
     "/rest/getAlbumList2.view"=>json!({"subsonic-response":{"status":"ok","albumList2":{"album":[album.clone()]}}}),
     "/rest/getAlbum.view"=>json!({"subsonic-response":{"status":"ok","album":album}}),
     "/identity"=>json!({"MediaContainer":{"machineIdentifier":"server-one"}}),
     "/library/sections"=>json!({"MediaContainer":{"Directory":[{"key":"1","type":"artist","title":"Music"}]}}),
     "/library/sections/1/all"=>json!({"MediaContainer":{"offset":0,"size":2,"totalSize":2,"Metadata":[{"ratingKey":"plex-album","title":"Plex Album","parentTitle":"Artist","year":2000,"addedAt":1793404800u64,"Genre":[{"tag":"Jazz"}],"thumb":"/library/metadata/plex-album/thumb"},{"ratingKey":"other","title":"Other","parentTitle":"Artist","year":1980,"Genre":[{"tag":"Rock"}]}]}}),
     _=>panic!("unexpected fixture endpoint {}",url.path()),
    }.to_string();
    let extra=if url.path().starts_with("/rest/"){assert!(!request.contains("PRIVATE_PASSWORD"));"X-Total-Count: 1\r\n"}else{assert!(request.to_ascii_lowercase().contains("x-plex-token: private_plex") || url.query_pairs().any(|(k,v)|k.eq_ignore_ascii_case("X-Plex-Token")&&v=="PRIVATE_PLEX"));""};
    let _=socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
   });
  }while let Some(result)=children.join_next().await{result.unwrap();}});
  let nav=crate::windows_navidrome::Controller::smart_test_host(dir.join("nav-account.json"),crate::windows_navidrome::api::Credentials{server:url.clone(),username:"fixture".into(),password:"PRIVATE_PASSWORD".into(),connection_id:"OLD".into()});
  nav.apply_enabled_from_config(true).await.unwrap();
  let plex=crate::windows_plex::Controller::smart_test_host(dir.join("plex-account.json"),crate::windows_plex::Settings{enabled:true,plex_url:url,plex_token:"PRIVATE_PLEX".into(),plex_server_id:Some("server-one".into()),plex_server_name:None});plex.apply_enabled_from_config(true).await.unwrap();
  let library=Library::new(dir.join("library.json"));library.configure(None,Sources{local:true,navidrome:true,plex:true}).unwrap();
  let store=smart::Controller::new(dir.join("smart.json")).unwrap();let favorites=Favorites::new(dir.join("favorites.json"));favorites.import("[]").unwrap();
  Self{dir,library,store,favorites,nav,plex,requests,stop,peer}
 }
 async fn call(&self,operation:&str,args:Value)->Result<Value,String>{run(operation.into(),args,self.store.clone(),self.library.clone(),self.favorites.clone(),self.nav.clone(),self.plex.clone()).await}
 async fn close(self){self.nav.begin_shutdown();self.plex.begin_shutdown();let n=self.nav;let p=self.plex;let s=self.store;let f=self.favorites;tokio::task::spawn_blocking(move||{n.shutdown().unwrap();p.shutdown().unwrap();s.shutdown().unwrap();f.shutdown().unwrap();}).await.unwrap();self.stop.send_replace(true);self.peer.await.unwrap();self.library.stop();std::fs::remove_dir_all(self.dir).unwrap();}
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn original_ipc_flow_catalog_preview_favorites_and_store_are_live_for_both_remote_sources(){
 let e=Environment::new().await;assert!(e.requests.lock().unwrap().is_empty(),"constructing/restoring sources is quiet");
 let sources=e.call("sources",json!({})).await.unwrap();let account=sources["navidrome_account_id"].as_str().unwrap().to_string();assert_eq!(sources["plex_server_id"],"server-one");assert_eq!(sources["local"],false);
 for source in [json!({"kind":"navidrome","account_id":account}),json!({"kind":"plex","server_id":"server-one","library_id":"1"})]{
  let catalog=e.call("catalog",json!({"source":source})).await.unwrap();assert!(!catalog.as_array().unwrap().is_empty());assert_eq!(catalog[0]["genres"],json!(["Jazz"]));assert_eq!(catalog[0]["added_at"],1793404800u64);
  let mut r=serde_json::to_value(rules(serde_json::from_value(source.clone()).unwrap())).unwrap();r["limit"]=json!(20);r["genre"]=json!("Jazz");
  let selection=e.call("preview",json!({"rules":r})).await.unwrap();assert_eq!(selection["matched_albums"],1);assert_eq!(selection["albums"][0]["genres"],json!(["Jazz"]));
  if source["kind"]=="plex"{assert_eq!(selection["albums"][0]["image"]["path"],"/library/metadata/plex-album/thumb");}
  let name=source["kind"].as_str().unwrap();let saved=e.call("store",json!({"action":{"kind":"save","id":null,"name":name,"rules":r}})).await.unwrap();assert!(saved.as_array().unwrap().iter().any(|p|p["name"]==name));
  r["favorites_only"]=json!(true);assert_eq!(e.call("preview",json!({"rules":r.clone()})).await.unwrap()["matched_albums"],0);
  if source["kind"]=="navidrome"{let status=e.nav.status().await.unwrap();e.nav.favorites(status.connection_id.unwrap(),Some("nav-album".into()),Some(true)).await.unwrap();}
  else{e.favorites.toggle(serde_json::from_value(json!({"id":"plex-album","source":"plex","title":"Plex Album","artist":"Artist","path_or_key":"plex-album","plex_server_id":"server-one"})).unwrap()).unwrap();}
  assert_eq!(e.call("preview",json!({"rules":r})).await.unwrap()["matched_albums"],1);
 }
 let bytes=std::fs::read(e.dir.join("smart.json")).unwrap();let text=String::from_utf8(bytes.clone()).unwrap();for forbidden in ["PRIVATE_PLEX","PRIVATE_PASSWORD","connection_id","127.0.0.1","media_locator"]{assert!(!text.contains(forbidden));}
 let reopened=smart::Controller::new(e.dir.join("smart.json")).unwrap();assert_eq!(reopened.call(smart::Action::List).unwrap().len(),2);reopened.shutdown().unwrap();assert_eq!(std::fs::read(e.dir.join("smart.json")).unwrap(),bytes);
 assert!(!e.requests.lock().unwrap().iter().any(|p|p.contains("stream")||p.contains("parts")));e.close().await;
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn source_epoch_rejects_same_account_aba_and_disabled_sources_without_media_effects(){
 let e=Environment::new().await;let account=e.nav.status().await.unwrap().account_id.unwrap();let nav=json!({"kind":"navidrome","account_id":account});let plex=json!({"kind":"plex","server_id":"server-one","library_id":"1"});
 for source in [nav,plex]{let token=e.call("epoch",json!({"source":source})).await.unwrap();
  if source["kind"]=="navidrome"{e.nav.apply_enabled_from_config(false).await.unwrap();e.nav.apply_enabled_from_config(true).await.unwrap();}else{e.plex.apply_enabled_from_config(false).await.unwrap();e.plex.apply_enabled_from_config(true).await.unwrap();}
  assert_eq!(e.call("epoch",json!({"source":source,"expected":token})).await.unwrap_err(),SOURCE);
  assert!(e.call("epoch",json!({"source":source})).await.is_ok());
 }
 e.library.configure(None,Sources{local:true,navidrome:false,plex:false}).unwrap();assert!(e.call("catalog",json!({"source":{"kind":"plex","server_id":"server-one","library_id":"1"}})).await.is_err());assert!(e.requests.lock().unwrap().is_empty());e.close().await;
}
