//! Original Plex IPC contracts, backed exclusively by the Windows account host.
use super::{Controller,core};
use crate::windows_mpd::session::Controller as Audio;
async fn ready(audio:&Audio)->Result<(),String>{let end=std::time::Instant::now()+std::time::Duration::from_secs(3);while !audio.ready(){if std::time::Instant::now()>=end{return Err(super::UNAVAILABLE.into());}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}Ok(())}
#[tauri::command]
pub(crate) async fn plex_create_pin(state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<core::PlexPin,String>{ready(&audio).await?;state.pin().await}
#[tauri::command]
pub(crate) async fn plex_check_pin(pin_id:u64,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Option<String>,String>{ready(&audio).await?;state.check_pin(pin_id).await}
#[tauri::command]
pub(crate) async fn plex_get_servers(auth_token:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::PlexServerResource>,String>{ready(&audio).await?;state.servers(auth_token).await}
#[tauri::command]
pub(crate) async fn get_plex_libraries(state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::catalog::PlexLibrary>,String>{ready(&audio).await?;state.libraries().await}
#[tauri::command]
pub(crate) async fn get_plex_collections(section_key:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::content::PlexCollection>,String>{ready(&audio).await?;state.collections(section_key).await}
#[tauri::command]
pub(crate) async fn get_collection_albums(rating_key:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::catalog::PlexAlbum>,String>{ready(&audio).await?;state.collection_albums(rating_key).await}
#[tauri::command]
pub(crate) async fn get_artist_portrait(server_id:String,rating_key:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Option<crate::plex::PlexImageRef>,String>{ready(&audio).await?;state.artist_portrait(server_id,rating_key).await}
#[tauri::command]
pub(crate) async fn get_artist_albums(rating_key:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::catalog::PlexAlbum>,String>{ready(&audio).await?;state.artist_albums(rating_key).await}
#[tauri::command]
pub(crate) async fn get_artist_top_tracks(rating_key:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::content::PlexTrack>,String>{ready(&audio).await?;state.artist_tracks(rating_key).await}
#[tauri::command]
pub(crate) async fn search_plex(query:String,section_key:Option<String>,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<core::content::PlexSearchResults,String>{ready(&audio).await?;state.search(query,section_key).await}
#[tauri::command]
pub(crate) async fn get_album_tracks(rating_key:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<Vec<core::content::PlexTrack>,String>{ready(&audio).await?;state.album_tracks(rating_key).await}
#[tauri::command]
pub(crate) async fn get_plex_discover(server_id:String,section_key:String,language:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexDiscoverPage,String>{ready(&audio).await?;state.discover(server_id,section_key,language).await}
#[tauri::command]
pub(crate) async fn get_plex_discover_recent(server_id:String,section_key:String,language:String,window_size:usize,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexDiscoverRecent,String>{ready(&audio).await?;state.discover_recent(server_id,section_key,language,window_size).await}
#[tauri::command]
pub(crate) async fn get_plex_discover_children(server_id:String,section_key:String,kind:String,rating_key:String,language:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexDiscoverHub,String>{ready(&audio).await?;state.discover_children(server_id,section_key,kind,rating_key,language).await}
#[tauri::command]
pub(crate) async fn get_plex_discover_artist(server_id:String,section_key:String,rating_key:String,language:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexDiscoverArtistContent,String>{ready(&audio).await?;state.discover_artist(server_id,section_key,rating_key,language).await}
#[tauri::command]
pub(crate) async fn get_plex_library_mix_years(server_id:String,section_key:String,language:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexMixYears,String>{ready(&audio).await?;state.mix_years(server_id,section_key,language).await}
#[tauri::command]
pub(crate) async fn get_plex_library_mix_styles(server_id:String,section_key:String,language:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexMixStyles,String>{ready(&audio).await?;state.mix_styles(server_id,section_key,language).await}
#[tauri::command]
pub(crate) async fn get_plex_library_mix(server_id:String,section_key:String,language:String,limit:usize,decade:Option<u32>,style:Option<super::PlexStyleSelection>,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<super::PlexLibraryMixPage,String>{ready(&audio).await?;state.mix(server_id,section_key,language,limit,decade,style).await}
#[tauri::command]
pub(crate) async fn get_plex_albums(section_key:String,sort_by:String,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<core::catalog::PlexAlbumCatalog,core::catalog::PlexAlbumCatalogError>{
 ready(&audio).await.map_err(|message|core::catalog::PlexAlbumCatalogError{message,received:0,completeness:core::catalog::AlbumCatalogCompleteness::Unknown})?;state.albums(section_key,sort_by).await
}
#[tauri::command]
pub(crate) async fn get_plex_image(image:crate::plex::PlexImageRef,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<tauri::ipc::Response,String>{ready(&audio).await?;Ok(tauri::ipc::Response::new(state.image(image).await.map_err(|error|match error.as_str(){super::CHANGED|super::INVALID=>error,_=>"PLEX_ARTWORK_UNAVAILABLE".into()})?))}
#[tauri::command]
pub(crate) async fn open_external_url(url:String,app:tauri::AppHandle)->Result<(),String>{
 use tauri_plugin_opener::OpenerExt;
 let parsed=reqwest::Url::parse(&url).map_err(|_|super::INVALID)?;
 if parsed.scheme()!="https"||parsed.host_str()!=Some("app.plex.tv")||parsed.port().is_some()||!parsed.username().is_empty()||parsed.password().is_some()||parsed.path()!="/auth"{return Err(super::INVALID.into());}
 app.opener().open_url(url,None::<&str>).map_err(|_|super::UNAVAILABLE.into())
}
#[tauri::command]
pub(crate) async fn resolve_player_plex_navigation(media:serde_json::Value,state:tauri::State<'_,Controller>,audio:tauri::State<'_,Audio>)->Result<serde_json::Value,String>{
 ready(&audio).await?;
 let index=media.get("queue_index").and_then(serde_json::Value::as_u64).and_then(|v|usize::try_from(v).ok()).ok_or(super::INVALID)?;
 let c=audio.inner().clone();let before=tauri::async_runtime::spawn_blocking(move||c.status()).await.map_err(|_|super::UNAVAILABLE)?.map_err(str::to_owned)?;
 let source=before.current_source(index).ok_or(super::CHANGED)?;
 let crate::windows_mpd::queue_v3::Source::Plex{server_id,part_key,rating_key}=source else{return Err(super::CHANGED.into())};
 if media.get("kind").and_then(serde_json::Value::as_str)!=Some("plex")||media.get("server_id").and_then(serde_json::Value::as_str)!=Some(server_id.as_str())||media.get("part_key").and_then(serde_json::Value::as_str)!=Some(part_key.as_str()){return Err(super::CHANGED.into());}
 let locator=crate::audio::MediaLocator::Plex{server_id:server_id.clone(),part_key:part_key.clone(),rating_key:Some(rating_key.clone()),file_path:None};
 let navigation=state.navigation(locator).await?;
 let c=audio.inner().clone();let after=tauri::async_runtime::spawn_blocking(move||c.status()).await.map_err(|_|super::UNAVAILABLE)?.map_err(str::to_owned)?;
 if before.generation!=after.generation||after.current_source(index)!=Some(source){return Err(super::CHANGED.into());}
 Ok(serde_json::json!({"media":media,"navigation":navigation}))
}
