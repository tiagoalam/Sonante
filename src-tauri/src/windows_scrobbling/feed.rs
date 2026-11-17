//! Pure projection of an actual serialized engine snapshot, never a UI clock.
use super::model::{Observation,Track};
use crate::windows_mpd::{library::{Track as QueueTrack,Sources},queue_v3::Source};
use sha2::{Digest,Sha256};
use std::time::{Instant,SystemTime,UNIX_EPOCH};
pub(crate) fn sample(running:bool,state:&str,uncertain:bool,elapsed:f64,duration:f64,index:usize,queue:Option<&QueueTrack>,sources:Sources)->Observation{
 let track=queue.filter(|q|q.radio.is_none()&&q.upnp.is_none()).and_then(|q|Track::new(&q.artist,&q.title,&q.album,duration));
 let key=queue.filter(|q|q.radio.is_none()&&q.upnp.is_none()).and_then(|q|{
  let source=match q.remote_source.as_ref(){
   Some(s@Source::Navidrome{..})if sources.navidrome=>{
    let lease=q.remote_lease.as_ref().filter(|lease|lease.healthy()&&lease.source()==s&&!lease.connection_id().is_empty())?;
    Some(format!("nav:{}:{}",serde_json::to_string(s).ok()?,lease.connection_id()))
   },
   Some(s@Source::Plex{..})if sources.plex&&q.remote_lease.as_ref().is_some_and(|lease|lease.healthy()&&lease.source()==s)=>serde_json::to_string(s).ok(),
   None if sources.local=>Some(format!("local:{}",q.id)),
   _=>None,
  }?;
  Some(format!("{:x}:{index}",Sha256::digest(source.as_bytes())))
 });
 let key=if running&&!uncertain&&track.is_some(){key}else{None};
 let allowed=queue.is_some_and(|q|q.radio.is_none()&&q.upnp.is_none()&&match q.remote_source.as_ref(){None=>sources.local,Some(Source::Navidrome{..})=>sources.navidrome,Some(Source::Plex{..})=>sources.plex,Some(Source::Local{..})=>false});
 let gap=uncertain||running&&allowed&&(track.is_none()||key.is_none());
 Observation{at:Instant::now(),utc:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0,|d|d.as_secs()),key,playing:running&&state=="play",stopped:!running||state=="stop",gap,elapsed,track}
}

#[cfg(test)]mod tests{
 use super::*;use std::sync::{Arc,atomic::{AtomicBool,Ordering}};use super::super::model::{Tracker,Target};
 struct Lease{source:Source,healthy:Arc<AtomicBool>,connection:String}
 impl crate::windows_mpd::library::RemoteLease for Lease{
  fn uri(&self)->&str{"http://127.0.0.1/PRIVATE-CAPABILITY"}fn source(&self)->&Source{&self.source}fn connection_id(&self)->&str{&self.connection}fn healthy(&self)->bool{self.healthy.load(Ordering::Acquire)}
  fn command(&self,_:&mut dyn FnMut()->crate::windows_mpd::protocol::Result<Vec<String>>)->crate::windows_mpd::protocol::Result<Vec<String>>{panic!("read-only snapshot projection must never send commands")}
 }
 fn queue(source:Option<Source>)->(QueueTrack,Arc<AtomicBool>){
  let healthy=Arc::new(AtomicBool::new(true));let lease=source.as_ref().map(|s|Arc::new(Lease{source:s.clone(),healthy:healthy.clone(),connection:"PRIVATE-SESSION".into()})as Arc<dyn crate::windows_mpd::library::RemoteLease>);
  (QueueTrack{id:"PRIVATE-PATH-IDENTITY".into(),title:"Song".into(),artist:"Artist".into(),album:"Album".into(),disc:1,number:1,file_name:"PRIVATE-FILENAME".into(),catalog_album_id:None,path:std::path::PathBuf::from("/not-opened-own-fixture"),media_root:None,radio:None,upnp:None,remote_source:source,remote_connection_id:Some("PRIVATE-SESSION".into()),plex_image:None,remote_lease:lease},healthy)
 }
 fn flags(mask:u8)->Sources{Sources{local:mask&1!=0,plex:mask&2!=0,navidrome:mask&4!=0}}
 #[test]fn existing_linux_source_flags_and_required_healthy_binding_are_preserved(){
  for (source,bit)in [(None,1),(Some(Source::Plex{server_id:"PRIVATE-SERVER".into(),part_key:"/library/parts/PRIVATE".into(),rating_key:"7".into()}),2),(Some(Source::Navidrome{account_id:"a".repeat(64),song_id:"PRIVATE-SONG".into()}),4)]{
   let(mut q,health)=queue(source);for mask in 0..8{assert_eq!(sample(true,"play",false,0.,120.,0,Some(&q),flags(mask)).key.is_some(),mask&bit!=0);}
   if q.remote_source.is_some(){health.store(false,Ordering::Release);let s=sample(true,"play",false,5.,120.,0,Some(&q),flags(7));assert!(s.gap);assert!(s.key.is_none());health.store(true,Ordering::Release);q.remote_lease=None;assert!(sample(true,"play",false,5.,120.,0,Some(&q),flags(7)).key.is_none());}
  }
 }
 #[test]fn navidrome_connection_replacement_is_a_new_observed_occurrence_like_linux(){
  let source=Source::Navidrome{account_id:"a".repeat(64),song_id:"own-song".into()};let(mut q,health)=queue(Some(source.clone()));
  let before=sample(true,"play",false,5.,120.,0,Some(&q),flags(7)).key.unwrap();
  q.remote_lease=Some(Arc::new(Lease{source,healthy:health,connection:"PRIVATE-NEW-SESSION".into()}));
  let after=sample(true,"play",false,5.,120.,0,Some(&q),flags(7)).key.unwrap();assert_ne!(before,after);assert!(!before.contains("PRIVATE"));assert!(!after.contains("PRIVATE"));
 }
 #[test]fn observed_elapsed_qualifies_once_and_private_locators_never_enter_pending_payloads(){
  let(q,_)=queue(Some(Source::Navidrome{account_id:"a".repeat(64),song_id:"PRIVATE-SONG".into()}));let origin=Instant::now();let targets=std::array::from_fn(|_|Some(Target{account:"own-service-fixture".into(),since:origin}));let mut tracker=Tracker::default();let mut pending=Vec::new();let mut now_playing=0;
  for second in 0..=70{let mut s=sample(true,"play",false,second as f64,120.,3,Some(&q),flags(7));s.at=origin+std::time::Duration::from_secs(second);s.utc=1000+second;let(np,jobs)=tracker.observe(s,&targets);if second<60{assert!(jobs.is_empty());}now_playing+=np.len();pending.extend(jobs);}
  assert_eq!(now_playing,2);assert_eq!(pending.len(),2);let raw=serde_json::to_string(&pending).unwrap();for value in ["PRIVATE-","127.0.0.1","song_id","account_id","file_name","path"]{assert!(!raw.contains(value));}
 }
 #[test]fn radio_and_unknown_local_as_remote_never_become_service_listens(){
  let(mut q,_)=queue(None);q.radio=Some(crate::windows_radio::Playing{station_id:"own-station".into(),thumb:None,stream_url:"https://not-contacted.invalid".into(),station_name:"Fixture".into()});
  let s=sample(true,"play",false,120.,120.,0,Some(&q),flags(7));assert!(s.key.is_none());assert!(s.track.is_none());assert!(!s.gap);
  q.radio=None;q.remote_source=Some(Source::Local{root:std::path::PathBuf::from("/not-opened"),path:std::path::PathBuf::from("/not-opened/file")});
  assert!(sample(true,"play",false,120.,120.,0,Some(&q),flags(7)).key.is_none());
 }
 #[test]fn upnp_receiver_never_becomes_a_local_service_listen(){
  let mut row=crate::windows_upnp::media::selection(crate::upnp::Media{id:"12345678-1234-4234-8234-123456789abc".into(),uri:"http://192.168.25.9/own.flac?key=PRIVATE".into(),metadata:String::new(),title:"Song".into(),artist:"Artist".into(),album:"Album".into(),artwork:None}).unwrap().tracks.remove(0);
  row.catalog_album_id=Some("not-authority".into());
  let value=sample(true,"play",false,60.,120.,0,Some(&row),flags(7));assert!(value.key.is_none());assert!(value.track.is_none());assert!(!value.gap);
 }
 #[test]fn errors_and_invalid_metadata_are_gaps_not_another_occurrence_after_recovery(){
  let(q,_)=queue(None);let origin=Instant::now();let targets=std::array::from_fn(|_|Some(Target{account:"fixture".into(),since:origin}));let mut tracker=Tracker::default();
  for second in 0..=30{let mut s=sample(true,"play",false,second as f64,60.,0,Some(&q),flags(7));s.at=origin+std::time::Duration::from_secs(second);tracker.observe(s,&targets);}
  let mut gap=sample(true,"play",true,31.,60.,0,Some(&q),flags(7));gap.at=origin+std::time::Duration::from_secs(31);assert!(tracker.observe(gap,&targets).1.is_empty());let mut s=sample(true,"play",false,32.,60.,0,Some(&q),flags(7));s.at=origin+std::time::Duration::from_secs(32);let(np,pending)=tracker.observe(s,&targets);assert!(np.is_empty());assert!(pending.is_empty());
  let s=sample(false,"pause",false,120.,120.,0,Some(&q),flags(7));assert!(!s.playing);assert!(s.stopped);assert!(s.key.is_none());
  let mut bad=q;bad.artist="Unknown Artist".into();let s=sample(true,"play",false,60.,120.,0,Some(&bad),flags(7));assert!(s.gap);assert!(s.track.is_none());assert!(s.key.is_none());
 }
}
