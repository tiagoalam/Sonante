//! Same complete account-bound catalog policy as Linux navidrome/library.rs.
use super::{api,connection::Connection,host::Controller,CHANGED,PROTOCOL};
use crate::windows_smart_playlists::{self as smart,Album};
use std::{collections::HashSet,time::{Duration,Instant}};
use serde_json::Value;
fn metadata(value:&Value)->Result<Album,String>{
 let a=api::album(value)?;
 let mut genres=Vec::new();
 if let Some(genre)=value["genre"].as_str(){genres.push(genre.to_string());}
 if let Some(values)=value["genres"].as_array(){for v in values.iter().take(64){if let Some(genre)=v["name"].as_str().or_else(||v.as_str()){genres.push(genre.to_string());}}}
 if genres.iter().any(|v|v.len()>256||v.chars().any(char::is_control)){return Err(PROTOCOL.into());}
 genres.sort();genres.dedup();
 let created=value["created"].as_str().filter(|v|v.ends_with('Z')||v.ends_with("+00:00")).and_then(|v|v.get(..19)).map(|v|format!("{v}Z"));
 Ok(Album{id:a.id.clone(),title:a.title,artist:a.artist,genres,year:a.year,added_at:smart::local_added(created.as_deref()),favorite_key:a.id,image:None})
}
pub(super) struct CatalogSnapshot{at:Instant,albums:Vec<Album>,presentation:Vec<api::Album>}
async fn populate(c:&Connection,cache:&mut Option<CatalogSnapshot>)->Result<(),String>{
 if !c.active(){return Err(CHANGED.into());}
 if cache.as_ref().is_some_and(|cache|cache.at.elapsed()<Duration::from_secs(30)){return Ok(());}
 let mut albums=Vec::new();let mut presentation=Vec::new();let mut ids=HashSet::new();let mut offset=0usize;let mut expected=None;
 loop{
  let(root,total)=c.request_counted("getAlbumList2",&[("type","alphabeticalByName".into()),("size","72".into()),("offset",offset.to_string())]).await?;
  if !c.active(){return Err(CHANGED.into());}
  if expected.is_some()&&total.is_some()&&total!=expected{return Err(PROTOCOL.into());}
  if expected.is_none(){expected=total;}
  let parent=root.get("albumList2").filter(|v|v.is_object()).ok_or(PROTOCOL)?;
  let rows=match parent.get("album"){None=>&[][..],Some(Value::Array(rows))if rows.len()<=72=>rows.as_slice(),_=>return Err(PROTOCOL.into())};
  let before=albums.len();
  for row in rows{let album=metadata(row)?;if ids.insert(album.id.clone()){presentation.push(api::album(row)?);albums.push(album);}}
  offset=offset.checked_add(rows.len()).ok_or(PROTOCOL)?;
  if albums.len()>100_000{return Err(PROTOCOL.into());}
  if rows.len()<72||expected.is_some_and(|n|offset>=n){break;}
  if albums.len()==before{return Err(PROTOCOL.into());}
 }
 if expected.is_some_and(|n|n!=albums.len()){return Err(PROTOCOL.into());}
 if !c.active(){return Err(CHANGED.into());}
 *cache=Some(CatalogSnapshot{at:Instant::now(),albums,presentation});Ok(())
}
impl Controller{
 pub(crate) async fn edition_catalog(&self,connection:String,account:String)->Result<Vec<api::Album>,String>{
  let registry=self.registry();let c=registry.for_account(&account)?;if c.credentials.connection_id!=connection{return Err(CHANGED.into());}
  self.catalog_read(async{let mut cache=c.smart_catalog.lock().await;populate(&c,&mut cache).await?;registry.with_current(&c,||Ok(cache.as_ref().ok_or(PROTOCOL)?.presentation.clone()))}).await
 }
 pub(crate) async fn edition_profile(&self,connection:String,account:String,album:String)->Result<Option<crate::windows_editions::policy::Profile>,String>{
  let registry=self.registry();let c=registry.for_account(&account)?;
  if c.credentials.connection_id!=connection||!api::id(&album){return Err(CHANGED.into());}
  self.read(async{let root=c.request("getAlbum",&[("id",album.clone())]).await?;registry.with_current(&c,||Ok(root.get("album").and_then(|v|crate::windows_editions::policy::navidrome_profile(v,&album))))}).await
 }
 pub(crate) fn smart_epoch(&self,account:&str)->Result<String,String>{let c=self.registry().for_account(account)?;Ok(c.credentials.connection_id.clone())}
 pub(crate) async fn smart_catalog(&self,account:String)->Result<Vec<Album>,String>{
  let registry=self.registry();let c=registry.for_account(&account)?;
  self.catalog_read(async{let mut cache=c.smart_catalog.lock().await;populate(&c,&mut cache).await?;registry.with_current(&c,||Ok(cache.as_ref().ok_or(PROTOCOL)?.albums.clone()))}).await
 }
 pub(crate) async fn smart_favorites(&self,account:String)->Result<Vec<String>,String>{
  let registry=self.registry();let c=registry.for_account(&account)?;
  let rows=self.favorites(c.credentials.connection_id.clone(),None,None).await?;
  registry.with_current(&c,||Ok(rows.into_iter().map(|a|a.id).collect()))
 }
}
#[cfg(test)]
#[path="smart_tests.rs"]
mod tests;
