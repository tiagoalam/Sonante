//! Original smart-playlist workflow, with explicit Windows source bindings.
//! No playback engine/listener is prepared while cataloging or saving rules.
use crate::{windows_smart_playlists as smart, windows_mpd::library::{Library,Sources},windows_favorites::Store as Favorites};
use serde_json::{Value,json};
use std::collections::HashSet;
const INVALID:&str="SMART_PLAYLIST_INVALID";
const SOURCE:&str="SMART_PLAYLIST_SOURCE";
const READ:&str="SMART_PLAYLIST_READ";
fn flags(library:&Library)->Result<Sources,String>{library.preferences().map(|(_,s)|s).map_err(|_|SOURCE.into())}
fn enabled(flags:Sources,source:&smart::Source)->bool{match source{smart::Source::Local{}=>flags.local,smart::Source::Navidrome{..}=>flags.navidrome,smart::Source::Plex{..}=>flags.plex}}
fn rules(source:smart::Source)->smart::Rules{smart::Rules{source,genre:None,genre_family:None,track_mode:smart::TrackMode::Album,artist:None,year_from:None,year_to:None,added_days:None,favorites_only:false,sort:smart::Sort::Album,limit:1}}
fn epoch(library:&Library,nav:&crate::windows_navidrome::Controller,plex:&crate::windows_plex::Controller,source:&smart::Source)->Result<String,String>{
 rules(source.clone()).validate()?;
 if !enabled(flags(library)?,source){return Err(SOURCE.into());}
 match source{
  smart::Source::Local{}=>smart::epoch(library,None),
  smart::Source::Navidrome{account_id}=>nav.smart_epoch(account_id).map_err(|_|SOURCE.into()),
  smart::Source::Plex{server_id,..}=>plex.smart_epoch(server_id).map_err(|_|SOURCE.into()),
 }
}
fn stale(source:&smart::Source)->String{if matches!(source,smart::Source::Local{}){READ.into()}else{SOURCE.into()}}
async fn catalog(library:&Library,nav:&crate::windows_navidrome::Controller,plex:&crate::windows_plex::Controller,source:&smart::Source)->Result<Vec<smart::Album>,String>{
 let before=epoch(library,nav,plex,source)?;
 let albums=match source{
  smart::Source::Local{}=>smart::catalog(library)?.1,
  smart::Source::Navidrome{account_id}=>nav.smart_catalog(account_id.clone()).await.map_err(|_|READ.to_owned())?,
  smart::Source::Plex{server_id,library_id}=>plex.smart_catalog(server_id.clone(),library_id.clone()).await.map_err(|_|READ.to_owned())?,
 };
 if epoch(library,nav,plex,source)?!=before{return Err(stale(source));}
 Ok(albums)
}
async fn saved_favorites(favorites:Favorites)->Result<Vec<crate::windows_favorites::Album>,String>{
 tokio::task::spawn_blocking(move||favorites.list().map(|r|r.unwrap_or_default()).map_err(|_|READ.to_owned())).await.map_err(|_|READ.to_owned())?
}
pub(crate) async fn run(operation:String,args:Value,store:smart::Controller,library:Library,favorites:Favorites,nav:crate::windows_navidrome::Controller,plex:crate::windows_plex::Controller)->Result<Value,String>{
 match operation.as_str(){
  "store"=>{
   let action=serde_json::from_value(args.get("action").cloned().ok_or(INVALID)?).map_err(|_|INVALID)?;
   let library=library.clone();
   tokio::task::spawn_blocking(move||{let rows=store.call(action)?;let enabled_sources=flags(&library)?;serde_json::to_value(rows.into_iter().filter(|d|enabled(enabled_sources,&d.rules.source)).collect::<Vec<_>>()).map_err(|_|"SMART_PLAYLIST_STORAGE".into())}).await.map_err(|_|READ.to_owned())?
  },
  "sources"=>{
   let enabled_sources=flags(&library)?;
   let account=if enabled_sources.navidrome{nav.status().await.map_err(|_|SOURCE)?.account_id}else{None};
   let server=if enabled_sources.plex{plex.public_settings().await.map_err(|_|SOURCE)?.server_id.filter(|id|plex.smart_epoch(id).is_ok())}else{None};
   let final_sources=flags(&library)?;
   Ok(json!({"local":final_sources.local&&library.configured_root().map_err(|_|SOURCE)?.is_some(),"plex_server_id":if final_sources.plex{server}else{None},"navidrome_account_id":if final_sources.navidrome{account}else{None}}))
  },
  "epoch"=>{
   let source:smart::Source=serde_json::from_value(args.get("source").cloned().unwrap_or_else(||json!({"kind":"local"}))).map_err(|_|SOURCE)?;
   let token=epoch(&library,&nav,&plex,&source)?;
   match args.get("expected"){None|Some(Value::Null)=>{},Some(Value::String(old))if *old==token=>{},Some(Value::String(_))=>return Err(stale(&source)),_=>return Err(INVALID.into())}
   Ok(json!(token))
  },
  "catalog"=>{
   let source=serde_json::from_value(args.get("source").cloned().ok_or(SOURCE)?).map_err(|_|SOURCE)?;
   serde_json::to_value(catalog(&library,&nav,&plex,&source).await?).map_err(|_|READ.into())
  },
  "preview"=>{
   let r:smart::Rules=serde_json::from_value(args.get("rules").cloned().ok_or(INVALID)?).map_err(|_|INVALID)?;r.validate()?;
   let before=epoch(&library,&nav,&plex,&r.source)?;
   let selection=if matches!(r.source,smart::Source::Local{}){
    let lib=library.clone();let r=r.clone();tokio::task::spawn_blocking(move||smart::preview(&lib,&favorites,r)).await.map_err(|_|READ.to_owned())??
   }else{
    let albums=catalog(&library,&nav,&plex,&r.source).await?;
    let mut ids=HashSet::new();
    if r.favorites_only{
     match &r.source{
      smart::Source::Navidrome{account_id}=>ids.extend(nav.smart_favorites(account_id.clone()).await.map_err(|_|READ.to_owned())?),
      smart::Source::Plex{server_id,..}=>{
       let saved=saved_favorites(favorites).await?;
       for album in &albums{if saved.iter().any(|f|f.source=="plex"&&f.plex_server_id.as_ref()==Some(server_id)&&f.path_or_key==album.id){ids.insert(album.id.clone());}}
      },smart::Source::Local{}=>unreachable!(),
     }
    }
    smart::select_remote(albums,&r,&ids)?
   };
   if epoch(&library,&nav,&plex,&r.source)?!=before{return Err(stale(&r.source));}
   serde_json::to_value(selection).map_err(|_|READ.into())
  },_=>Err(INVALID.into())
 }
}

#[cfg(test)]
#[path="windows_smart_remote_tests.rs"]
mod tests;
