use super::*;
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
#[test]
fn original_metadata_genres_and_verified_utc_dates_are_preserved(){
 let a=metadata(&serde_json::json!({"id":"a","name":"Album","artist":"Artist","genre":"Jazz","genres":[{"name":"Blues"},"Jazz"],"created":"2026-10-31T00:00:00+00:00","year":1999})).unwrap();
 assert_eq!(a.genres,vec!["Blues","Jazz"]);assert_eq!(a.added_at,Some(1793404800));assert_eq!(a.year,Some(1999));
 for date in ["2026-02-29T00:00:00Z","2026-01-01T00:00:00-03:00","PRIVATE_PATH","2026-01-01"]{assert!(metadata(&serde_json::json!({"id":"a","name":"Album","created":date})).unwrap().added_at.is_none());}
 assert!(metadata(&serde_json::json!({"id":"a","name":"Album","genre":"bad\nsecret"})).is_err());
}
async fn peer(mode:&'static str)->(Arc<Connection>,tokio::sync::watch::Sender<bool>,tokio::task::JoinHandle<()>,Arc<AtomicUsize>){
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
 let count=Arc::new(AtomicUsize::new(0));let seen=count.clone();let(stop,mut receive)=tokio::sync::watch::channel(false);
 let task=tokio::spawn(async move{loop{
  let accepted=tokio::select!{_=receive.changed()=>break,r=listener.accept()=>r};let(mut socket,_)=accepted.unwrap();
  let mut request=Vec::new();while !request.ends_with(b"\r\n\r\n"){request.push(socket.read_u8().await.unwrap());assert!(request.len()<8192);}
  let request=String::from_utf8(request).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let url=reqwest::Url::parse(&format!("http://{address}{target}")).unwrap();assert_eq!(url.path(),"/rest/getAlbumList2.view");
  let query:std::collections::HashMap<_,_>=url.query_pairs().collect();assert_eq!(query.get("type").unwrap(),"alphabeticalByName");assert_eq!(query.get("size").unwrap(),"72");assert!(query.contains_key("t"));assert!(!request.contains("PRIVATE_PASSWORD"));
  let offset=query["offset"].parse::<usize>().unwrap();seen.fetch_add(1,Ordering::SeqCst);
  let range=if mode=="repeat"{0..72}else if offset==0{0..72}else{72..73};
  let rows:Vec<_>=range.map(|n|serde_json::json!({"id":format!("a{n}"),"name":format!("Album {n}"),"genre":"Jazz","created":"2026-10-31T00:00:00Z"})).collect();
  let body=if mode=="missing"{serde_json::json!({"subsonic-response":{"status":"ok"}})}else{serde_json::json!({"subsonic-response":{"status":"ok","albumList2":{"album":if mode=="short"{rows[..1].to_vec()}else{rows}}}})}.to_string();
  let total=match mode{"repeat"=>String::new(),"changes" if offset>0=>"X-Total-Count: 74\r\n".into(),_=>"X-Total-Count: 73\r\n".into()};
  let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{total}Connection: close\r\n\r\n{body}",body.len());let _=socket.write_all(response.as_bytes()).await;
 }});
 let credentials=api::Credentials{server:format!("http://{address}/"),username:"fixture".into(),password:"PRIVATE_PASSWORD".into(),connection_id:"SESSION".into()};let c=Connection::fixture(credentials).await;
 (c,stop,task,count)
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn complete_counted_catalog_is_cached_but_never_starts_playback_listener(){
 let(c,stop,task,count)=peer("good").await;let mut cache=None;
 populate(&c,&mut cache).await.unwrap();assert_eq!(cache.as_ref().unwrap().albums.len(),73);assert_eq!(count.load(Ordering::SeqCst),2);
 populate(&c,&mut cache).await.unwrap();assert_eq!(count.load(Ordering::SeqCst),2);assert!(!c.playback_listener_started());
 c.retire();assert!(populate(&c,&mut cache).await.is_err());stop.send_replace(true);task.await.unwrap();
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn changing_totals_short_catalog_missing_envelope_and_repeated_pages_never_publish_cache(){
 for mode in ["changes","short","missing","repeat"]{
  let(c,stop,task,count)=peer(mode).await;let mut cache=None;assert!(populate(&c,&mut cache).await.is_err(),"{mode}");assert!(cache.is_none());assert!(!c.playback_listener_started());assert!(count.load(Ordering::SeqCst)<=2);stop.send_replace(true);task.await.unwrap();
 }
}
