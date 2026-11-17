//! Original smart catalog projection, fenced to one Windows server binding.
use super::{host::Controller,core,CHANGED};
use crate::windows_smart_playlists::Album;
impl Controller{
 pub(crate) fn smart_epoch(&self,server:&str)->Result<String,String>{
  core::validate_plex_server_id(server).map_err(|_|CHANGED)?;
  let b=self.registry().selected()?;
  if b.settings.plex_server_id.as_deref()!=Some(server){return Err(CHANGED.into());}
  Ok(b.generation.clone())
 }
 pub(crate) async fn smart_catalog(&self,server:String,library:String)->Result<Vec<Album>,String>{
  let before=self.smart_epoch(&server)?;
  let result=self.albums(library.clone(),"titleSort:asc".into()).await.map_err(|_|"SMART_PLAYLIST_READ".to_owned())?;
  if result.server_id!=server||result.library_id!=library||result.completeness!=core::catalog::AlbumCatalogCompleteness::Complete||self.smart_epoch(&server)?!=before{return Err(CHANGED.into());}
  let binding=self.registry().selected()?;
  let rows=result.albums.into_iter().map(|a|{
   let image=a.thumb.filter(|i|i.server_id==server&&core::validate_favorite_image(&i.path).is_ok()&&(binding.settings.plex_token.is_empty()||!i.path.contains(&binding.settings.plex_token)));
   Album{id:a.rating_key.clone(),favorite_key:a.rating_key,title:a.title,artist:a.artist,genres:a.genres,year:a.year.filter(|y|(1000..=9999).contains(y)),added_at:a.added_at,image}
  }).collect();
  if self.smart_epoch(&server)?!=before{return Err(CHANGED.into());}Ok(rows)
 }
}
