//! Resolve stable Plex references outside the foreground MPD worker. Each queue
//! occurrence retains a lease; no authenticated URL is serialized or returned.
use super::{host::{Controller,Binding,Part,Registry},CHANGED,INVALID,UNAVAILABLE};
use crate::windows_mpd::{library::{Track,RemoteLease},queue_v3::Source};
use std::sync::Arc;
struct Lease{registry:Arc<Registry>,binding:Arc<Binding>,part:Arc<Part>,duration:Option<f64>}
impl RemoteLease for Lease {
 fn uri(&self)->&str{&self.part.uri}
 fn source(&self)->&Source{&self.part.source}
 fn connection_id(&self)->&str{&self.binding.generation}
 fn presentation_duration(&self)->Option<f64>{self.duration}
 fn healthy(&self)->bool{self.binding.active()}
 fn health_signal(&self)->Option<Arc<std::sync::atomic::AtomicBool>>{Some(self.binding.health_signal())}
 fn command(&self,effect:&mut dyn FnMut()->crate::windows_mpd::protocol::Result<Vec<String>>)->crate::windows_mpd::protocol::Result<Vec<String>>{
  self.registry.with_current(&self.binding,||effect().map_err(str::to_owned)).map_err(|e|super::playback_error(&e))
 }
}
impl Controller {
 pub(crate) fn bind_locator(&self,locator:&crate::audio::MediaLocator)->Result<Source,String>{
  let source=Source::from_locator(locator).map_err(|_|INVALID)?;
  let Source::Plex{server_id,rating_key,part_key}=&source else{return Err(INVALID.into())};
  super::core::content::validate_reference(server_id,rating_key,part_key)?;
  let binding=self.registry().selected()?;
  if binding.client.manager.selected_server_id().as_ref()!=Some(server_id){return Err(CHANGED.into());}
  Ok(source)
 }
 // Same presentation-only admission as Linux sanitize_track_artwork; the
 // locator remains the sole authority for media. No HTTP or lease is created.
 pub(crate) fn presentation_image(&self,source:&Source,image:Option<crate::plex::PlexImageRef>)->Option<crate::plex::PlexImageRef>{
  let image=image?;if !crate::windows_mpd::queue_v4::valid_image(source,&image){return None;}
  let binding=self.registry().selected().ok()?;let Source::Plex{server_id,..}=source else{return None;};
  if binding.settings.plex_server_id.as_ref()!=Some(server_id){return None;}
  let token=&binding.settings.plex_token;if !token.is_empty()&&image.path.contains(token){return None;}Some(image)
 }
 pub(crate) async fn prepare_tracks(&self,mut tracks:Vec<Track>)->Result<Vec<Track>,String>{
  if tracks.is_empty()||tracks.len()>1000{return Err(INVALID.into());}
  let registry=self.registry();let binding=registry.selected()?;
  self.read(async {
   let mut work=std::collections::VecDeque::new();
   for(index,track)in tracks.iter_mut().enumerate(){
    let Some(source@Source::Plex{..})=track.remote_source.as_ref()else{continue;};
    if track.radio.is_some()||!track.path.as_os_str().is_empty()||track.media_root.is_some(){return Err(INVALID.into());}
    if let Some(lease)=&track.remote_lease{if lease.source()==source&&lease.healthy()&&lease.connection_id()==binding.generation{continue;}}
    work.push_back((index,source.clone()));
   }
   let mut pending:tokio::task::JoinSet<Result<(usize,Source,Option<crate::plex::PlexImageRef>,Option<f64>),String>>=tokio::task::JoinSet::new();
   loop {
    while pending.len()<4{let Some((index,source))=work.pop_front()else{break};let binding=binding.clone();pending.spawn(async move{let verified=binding.client.inspect_reference(&source).await?;if verified.exists==Some(false){Err("PLEX_MEDIA_MISSING".into())}else{Ok((index,source,verified.image,verified.duration))}});}
    let Some(result)=pending.join_next().await else{break};let(index,source,image,duration)=result.map_err(|_|UNAVAILABLE)??;
    let part=registry.with_current(&binding,||binding.admit(source))?;
    tracks[index].plex_image=image;
    tracks[index].remote_connection_id=Some(binding.generation.clone());tracks[index].remote_lease=Some(Arc::new(Lease{registry:registry.clone(),binding:binding.clone(),part,duration}));
   }
   registry.with_current(&binding,||Ok(()))?;Ok(tracks)
  }).await
 }
 pub(crate) async fn available(&self,locator:crate::audio::MediaLocator)->Result<Option<bool>,String>{
  let source=self.bind_locator(&locator)?;let registry=self.registry();let binding=registry.selected()?;
  self.read(async{let value=binding.client.verify_reference(&source).await?;registry.with_current(&binding,||Ok(value))}).await
 }
 pub(crate) async fn available_many(&self,items:Vec<(usize,crate::audio::MediaLocator)>)->Vec<(usize,Result<Option<bool>,String>)>{
  if items.len()>1000{return items.into_iter().map(|(i,_)|(i,Err(INVALID.into()))).collect();}
  let all=items.iter().map(|(i,_)|*i).collect::<Vec<_>>();let mut todo=std::collections::VecDeque::from(items);let mut pending=tokio::task::JoinSet::new();let mut result=Vec::new();
  let round=async{loop{while pending.len()<4{let Some((index,locator))=todo.pop_front()else{break};let host=self.clone();pending.spawn(async move{(index,host.available(locator).await)});}let Some(value)=pending.join_next().await else{break};if let Ok(value)=value{result.push(value)}else{break;}}};
  let _=tokio::time::timeout(std::time::Duration::from_secs(16),round).await;pending.abort_all();for index in all{if !result.iter().any(|(i,_)|*i==index){result.push((index,Err(UNAVAILABLE.into())));}}result
 }
}
