use super::*;
use std::{fs,path::{Path,PathBuf},time::Instant};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
const ID:&str="12345678-1234-1234-1234-123456789abc";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0; 12];
        getrandom::fill(&mut bytes).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-win-catalog-{bytes:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn wave(path: &Path, title: &str, album: &str) {
    wave_tags(path, title, album, "Artista", "")
}
fn wave_tags(path: &Path, title: &str, album: &str, artist: &str, year: &str) {
    let mut body = b"WAVEfmt ".to_vec();
    body.extend(16u32.to_le_bytes());
    body.extend([1, 0, 2, 0]);
    body.extend(44100u32.to_le_bytes());
    body.extend(176400u32.to_le_bytes());
    body.extend([4, 0, 16, 0]);
    body.extend(b"data\0\0\0\0");
    let mut info = b"INFO".to_vec();
    for (name, value) in [
        (b"INAM", title),
        (b"IPRD", album),
        (b"IART", artist),
        (b"ICRD", year),
    ] {
        let mut value = value.as_bytes().to_vec();
        value.push(0);
        info.extend(name);
        info.extend((value.len() as u32).to_le_bytes());
        info.extend(&value);
        if value.len() % 2 != 0 {
            info.push(0)
        }
    }
    body.extend(b"LIST");
    body.extend((info.len() as u32).to_le_bytes());
    body.extend(info);
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    fs::write(path, file).unwrap();
}

fn ready(l:&Library){let end=Instant::now()+Duration::from_secs(5);while l.status().unwrap().phase!="ready"{assert!(Instant::now()<end);std::thread::sleep(Duration::from_millis(10));}}
fn indexed(f:&Fixture)->Library{let media=f.0.join("music");fs::create_dir(&media).unwrap();wave(&media.join("a.wav"),"One","Own Album");let l=Library::new(f.0.join("settings.json"));l.select(media).unwrap();ready(&l);l.configure_features(None,l.sources().unwrap(),None,Some(true)).unwrap();l.complete_first_run().unwrap();l}
fn request(l:&Library)->OnlineAlbumCoverRequest{let album=l.albums("",0,500).unwrap().items[0].clone();OnlineAlbumCoverRequest{album_id:album.id,title:album.title,artist:album.artist,year:album.year.map(|n|n.to_string())}}
#[test]fn artwork_preference_survives_legacy_config_refresh_and_reopen_without_enabling_beta(){
 let f=Fixture::new();let l=indexed(&f);assert!(l.online_artwork_enabled().unwrap());assert!(!l.experimental_features().unwrap());let scope=l.online_artwork_context(&request(&l).album_id).unwrap();l.configure(None,l.sources().unwrap()).unwrap();assert!(l.online_artwork_enabled().unwrap());assert!(l.check_online_artwork_context(&scope.0,scope.1).is_err());l.refresh().unwrap();ready(&l);assert!(l.online_artwork_enabled().unwrap());l.stop();let reopened=Library::new(f.0.join("settings.json"));ready(&reopened);assert!(reopened.online_artwork_enabled().unwrap());assert!(!reopened.experimental_features().unwrap());reopened.stop();
}
struct Server{base:String,stop:tokio::sync::watch::Sender<bool>,task:tokio::task::JoinHandle<()>,calls:Arc<AtomicUsize>,entered:Arc<tokio::sync::Notify>,release:Arc<tokio::sync::Notify>}
impl Server{
 async fn new(mode:u8)->Self{
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let calls=Arc::new(AtomicUsize::new(0));let seen=calls.clone();let(stop,mut stopped)=tokio::sync::watch::channel(false);let entered=Arc::new(tokio::sync::Notify::new());let admitted=entered.clone();let release=Arc::new(tokio::sync::Notify::new());let released=release.clone();
  let task=tokio::spawn(async move{loop{let(mut stream,_)=tokio::select!{_=stopped.changed()=>break,result=listener.accept()=>result.unwrap()};let mut bytes=Vec::new();while !bytes.ends_with(b"\r\n\r\n"){bytes.push(stream.read_u8().await.unwrap());assert!(bytes.len()<8192);}let request=String::from_utf8(bytes).unwrap();let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();let url=reqwest::Url::parse(&format!("http://{addr}{target}")).unwrap();seen.fetch_add(1,Ordering::AcqRel);
   let(status,mime,body)=if url.path()=="/mb"{if mode==4{admitted.notify_one();tokio::time::timeout(Duration::from_secs(3),released.notified()).await.unwrap();}if mode==2{("200 OK","application/json",b"broken".to_vec())}else if mode==5{("200 OK","application/json",vec![b'x';MAX_JSON+1])}else{assert!(url.query_pairs().any(|(k,v)|k=="query"&&v.contains("Own Album")&&v.contains("Artista")));("200 OK","application/json",serde_json::json!({"release-groups":[{"id":ID,"title":"Own Album","score":95,"artist-credit":[{"name":"Artista"}]}]}).to_string().into_bytes())}}
   else{assert_eq!(url.path(),format!("/art/{ID}/front-500"));if mode==1{("404 Not Found","image/jpeg",Vec::new())}else if mode==3{("200 OK","image/jpeg",b"wrong magic".to_vec())}else{("200 OK","image/jpeg",vec![255,216,255,1,2,3])}};
   let _=stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await;let _=stream.write_all(&body).await;
  }});Self{base:format!("http://{addr}"),stop,task,calls,entered,release}
 }
 async fn finish(self){self.stop.send_replace(true);self.task.await.unwrap();}
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]async fn real_http_cache_only_positive_and_confirmed_404_negative_without_engines_or_music_writes(){
 for mode in [0,1]{let f=Fixture::new();let l=indexed(&f);let s=Server::new(mode).await;let before=fs::read(f.0.join("music/a.wav")).unwrap();
 assert!(cover_at(l.clone(),request(&l),true,f.0.clone(),&format!("{}/mb",s.base),&format!("{}/art",s.base),false).await.unwrap().is_none());assert_eq!(s.calls.load(Ordering::Acquire),0);
 let result=cover_at(l.clone(),request(&l),false,f.0.clone(),&format!("{}/mb",s.base),&format!("{}/art",s.base),false).await.unwrap();assert_eq!(result.is_some(),mode==0);assert_eq!(s.calls.load(Ordering::Acquire),2);
 let cached=cover_at(l.clone(),request(&l),false,f.0.clone(),&format!("{}/mb",s.base),&format!("{}/art",s.base),false).await.unwrap();assert_eq!(cached,result);assert_eq!(s.calls.load(Ordering::Acquire),2);assert_eq!(fs::read(f.0.join("music/a.wav")).unwrap(),before);
 l.configure_features(None,l.sources().unwrap(),None,Some(false)).unwrap();assert!(cover_at(l.clone(),request(&l),false,f.0.clone(),&format!("{}/mb",s.base),&format!("{}/art",s.base),false).await.unwrap().is_none());assert_eq!(s.calls.load(Ordering::Acquire),2);l.stop();s.finish().await;}
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]async fn malformed_oversized_or_nonimage_responses_never_create_negative_records(){
 for mode in [2,3,5]{let f=Fixture::new();let l=indexed(&f);let s=Server::new(mode).await;assert!(cover_at(l.clone(),request(&l),false,f.0.clone(),&format!("{}/mb",s.base),&format!("{}/art",s.base),false).await.is_err());assert!(!f.0.join("windows-online-artwork-v1").exists());l.stop();s.finish().await;}
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]async fn disabling_during_http_cancels_without_cover_request_cache_write_or_false_negative(){
 let f=Fixture::new();let l=indexed(&f);let s=Server::new(4).await;let library=l.clone();let root=f.0.clone();let req=request(&l);let mb=format!("{}/mb",s.base);let art=format!("{}/art",s.base);let reading=tokio::spawn(async move{cover_at(library,req,false,root,&mb,&art,false).await});tokio::time::timeout(Duration::from_secs(3),s.entered.notified()).await.unwrap();l.configure_features(None,l.sources().unwrap(),None,Some(false)).unwrap();assert!(reading.await.unwrap().is_err());s.release.notify_one();assert_eq!(s.calls.load(Ordering::Acquire),1);assert!(!f.0.join("windows-online-artwork-v1").exists());l.stop();s.finish().await;
}
#[test]fn only_current_canonical_local_album_metadata_admits_a_read_and_reenable_never_revives_old_scope(){
 let f=Fixture::new();let l=indexed(&f);let mut bad=request(&l);bad.title="Other Album".into();assert!(Scope::admit(l.clone(),bad,f.0.clone()).is_err());let mut bad=request(&l);bad.year=Some("1990".into());assert!(Scope::admit(l.clone(),bad,f.0.clone()).is_err());let mut bad=request(&l);bad.album_id="/raw/private/path".into();assert!(Scope::admit(l.clone(),bad,f.0.clone()).is_err());
 let scope=Scope::admit(l.clone(),request(&l),f.0.clone()).unwrap();assert!(scope.current());l.configure_features(None,l.sources().unwrap(),None,Some(false)).unwrap();assert!(!scope.current());l.configure_features(None,l.sources().unwrap(),None,Some(true)).unwrap();assert!(!scope.current());let current=Scope::admit(l.clone(),request(&l),f.0.clone()).unwrap();assert!(current.current());l.stop();assert!(!current.current());
}
