//! Windows local-playback pilot. Linux retains its established module tree.
use crate::windows_mpd::library::{Albums, Library, Status as LibraryStatus, Track};
use crate::windows_mpd::{
    protocol,
    session::{Action, Controller, Snapshot},
};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Emitter, Manager};
#[path="windows_ab/mod.rs"]
mod album_ab;
#[path="windows_analyzer/mod.rs"]
mod audio_analyzer;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowsBootstrap {
    version: &'static str,
    milestone: &'static str,
    platform: &'static str,
    architecture: &'static str,
    playback_available: bool,
    runtime_version: String,
}

#[derive(Serialize)]
struct WindowsPreferences {usage_mode:crate::windows_mpd::library::UsageMode,online_artwork_enabled:bool,experimental_features:bool,first_run:bool,roots:Vec<String>,sources:crate::windows_mpd::library::Sources,plex:crate::windows_plex::PublicSettings,audio:crate::windows_audio::Settings}
#[tauri::command]
async fn get_online_album_cover(request:crate::windows_online_artwork::OnlineAlbumCoverRequest,cache_only:Option<bool>,library:tauri::State<'_,Library>)->Result<Option<String>,String>{crate::windows_online_artwork::cover(library.inner().clone(),request,cache_only.unwrap_or(false)).await}
#[tauri::command]
async fn get_online_cover_cache_status(request:crate::windows_online_artwork::OnlineAlbumCoverRequest,library:tauri::State<'_,Library>)->Result<String,String>{crate::windows_online_artwork::status(library.inner().clone(),request).await}
#[tauri::command]
fn get_edition_cache_namespace()->String{
 use sha2::{Digest,Sha256};format!("{:x}",Sha256::digest(crate::persistence::sonante_config_dir().as_os_str().as_encoded_bytes()))
}
#[tauri::command]
async fn get_edition_profile(source:crate::windows_editions::policy::Request,library:tauri::State<'_,Library>,nav:tauri::State<'_,crate::windows_navidrome::Controller>,plex:tauri::State<'_,crate::windows_plex::Controller>)->Result<Option<crate::windows_editions::policy::Profile>,String>{
 use crate::windows_editions::policy::Request;
 let lib=library.inner().clone();let scope=lib.edition_scope().map_err(str::to_owned)?;
 let result=match source{
  Request::Local{album_id}=>{if !scope.1.local{return Err("SOURCE_DISABLED".into());}let local=lib.clone();tauri::async_runtime::spawn_blocking(move||crate::windows_editions::local(local,album_id,scope)).await.map_err(|_|"EDITIONS_SOURCE")?},
  Request::Plex{server_id,library_id,album_id}=>{if !scope.1.plex{return Err("SOURCE_DISABLED".into());}plex.edition_profile(server_id,library_id,album_id).await},
  Request::Navidrome{connection_id,account_id,album_id}=>{if !scope.1.navidrome{return Err("SOURCE_DISABLED".into());}nav.edition_profile(connection_id,account_id,album_id).await},
 };
 if lib.edition_scope().ok()!=Some(scope){return Err("EDITIONS_SOURCE".into());}result
}
#[tauri::command]
async fn navidrome_edition_catalog(connection_id:String,account_id:String,library:tauri::State<'_,Library>,nav:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<Vec<crate::windows_navidrome::api::Album>,String>{
 let scope=library.edition_scope().map_err(str::to_owned)?;if !scope.1.navidrome{return Err("SOURCE_DISABLED".into());}
 let result=nav.edition_catalog(connection_id,account_id).await;
 if library.edition_scope().ok()!=Some(scope){return Err("EDITIONS_SOURCE".into());}result
}
#[tauri::command]
async fn get_local_track_tags(path:String,state:tauri::State<'_,Library>)->Result<crate::windows_metadata::types::LocalTrackTags,crate::windows_metadata::types::LocalTrackTagsError>{
 let library=state.inner().clone();tauri::async_runtime::spawn_blocking(move||crate::windows_metadata::inspect_track(library,path)).await.map_err(|_|crate::windows_metadata::types::LocalTrackTagsError::task_failed())?
}
#[tauri::command]
async fn get_local_album_tag_inspection(album_id:String,state:tauri::State<'_,Library>)->Result<crate::windows_metadata::types::LocalAlbumTagInspection,crate::windows_metadata::types::LocalTrackTagsError>{
 let library=state.inner().clone();tauri::async_runtime::spawn_blocking(move||crate::windows_metadata::inspect_album(library,album_id)).await.map_err(|_|crate::windows_metadata::types::LocalTrackTagsError::task_failed())?
}
#[tauri::command]
async fn get_local_album_doctor(album_id:String,state:tauri::State<'_,Library>)->Result<crate::windows_metadata::doctor::Report,crate::windows_metadata::doctor::DoctorError>{
 let library=state.inner().clone();tauri::async_runtime::spawn_blocking(move||crate::windows_metadata::diagnose_album(library,album_id)).await.map_err(|_|crate::windows_metadata::doctor::DoctorError::TaskFailed)?
}
#[tauri::command]
async fn get_online_album_metadata_details(mbid:String)->Result<crate::online_metadata::ReleaseDetails,crate::online_metadata::SearchError>{crate::online_metadata::lookup(mbid).await}
#[tauri::command]
async fn search_online_album_metadata(request:crate::online_metadata::SearchRequest)->Result<crate::online_metadata::SearchResult,crate::online_metadata::SearchError>{crate::online_metadata::search(request).await}
#[tauri::command]
async fn windows_preferences(state:tauri::State<'_,Controller>)->Result<WindowsPreferences,String>{let c=state.inner().clone();blocking(move||{let (roots,sources,plex,audio,first_run,experimental_features,online_artwork_enabled,usage_mode)=c.preferences_modes()?;Ok(WindowsPreferences{usage_mode,online_artwork_enabled,experimental_features,first_run,roots:roots.into_iter().map(ui_path).collect(),sources,plex,audio})}).await}
#[tauri::command]
async fn windows_audio_devices()->Result<Vec<crate::windows_audio::devices::Device>,String>{blocking(crate::windows_audio::devices::list).await}
#[tauri::command]
async fn windows_confirm_direct(expected:crate::windows_audio::Settings,intent:String,state:tauri::State<'_,Controller>)->Result<(),String>{let c=state.inner().clone();blocking(move||c.confirm_direct(expected,&intent)).await}
#[tauri::command]
async fn windows_direct_mute(muted:bool,token:String,state:tauri::State<'_,Controller>)->Result<(),String>{
 let c=state.inner().clone();blocking(move||c.direct_mute(muted,token).map(|_|())).await
}
#[tauri::command]
async fn windows_gain(action:Action,state:tauri::State<'_,Controller>)->Result<Snapshot,String>{
 let c=state.inner().clone();blocking(move||c.gain(None,action)).await
}
#[tauri::command]
async fn windows_sources(state:tauri::State<'_,Controller>)->Result<crate::windows_mpd::library::Sources,String>{let c=state.inner().clone();blocking(move||c.sources()).await}
#[tauri::command]
async fn windows_configure_sources(app:tauri::AppHandle,roots:Option<Vec<String>>,sources:crate::windows_mpd::library::Sources,plex_account:Option<crate::windows_plex::AccountDraft>,audio:Option<crate::windows_audio::Settings>,complete_setup:Option<bool>,experimental_features:Option<bool>,online_artwork_enabled:Option<bool>,usage_mode:Option<crate::windows_mpd::library::UsageMode>,plex:tauri::State<'_,crate::windows_plex::Controller>,state:tauri::State<'_,Controller>)->Result<crate::windows_mpd::library::imports::SaveResponse,String>{
 let c=state.inner().clone();let intent=c.admit_intent();let prepared=match plex_account{Some(draft)=>Some(plex.prepare_draft(sources.plex,draft).await?),None=>None};
 let response=blocking(move||c.configure_preferences_response(roots.map(|roots|roots.into_iter().map(std::path::PathBuf::from).collect()),sources,prepared,audio,complete_setup.unwrap_or(false),experimental_features,online_artwork_enabled,usage_mode,&intent)).await?;
 crate::windows_remote::preferences_changed(&app,experimental_features==Some(false));
 crate::windows_upnp::preferences_changed(&app);
 if app.emit("navidrome-configuration-changed",()).is_err(){eprintln!("Windows sources: notification_unavailable");}Ok(response)
}
#[tauri::command]
async fn windows_config_save_operation(state:tauri::State<'_,Library>)->Result<Option<crate::windows_mpd::library::imports::Snapshot>,String>{state.imports.snapshot().map_err(str::to_owned)}
#[tauri::command]
async fn windows_local_import_preview(operation_id:String,state:tauri::State<'_,Library>)->Result<crate::windows_mpd::library::imports::Preview,String>{
 let lease=state.imports.preview(&operation_id).map_err(str::to_owned)?;
 // The physical reservation is moved into the blocking task. Superseding an
 // operation revokes its authority, never the old queued reader's capacity.
 blocking(move||lease.read()).await
}
#[tauri::command]
async fn windows_local_import_artwork(operation_id:String,reference:String,state:tauri::State<'_,Library>)->Result<Option<String>,String>{
 let lease=state.imports.artwork(&operation_id,&reference).map_err(str::to_owned)?;let library=state.inner().clone();
 blocking(move||library.import_artwork(lease)).await
}
#[tauri::command]
async fn navidrome_configuration(state:tauri::State<'_,Controller>)->Result<bool,String>{let c=state.inner().clone();blocking(move||Ok(c.sources()?.navidrome)).await}
#[tauri::command]
async fn navidrome_status(state:tauri::State<'_,crate::windows_navidrome::Controller>,audio:tauri::State<'_,Controller>)->Result<crate::windows_navidrome::Status,String>{
 let end=std::time::Instant::now()+std::time::Duration::from_secs(3);
 while !audio.ready(){if std::time::Instant::now()>=end{return Err("NAVIDROME_UNAVAILABLE".into());}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
 state.status().await
}
#[tauri::command]
async fn navidrome_connect(app:tauri::AppHandle,server:String,username:String,password:String,state:tauri::State<'_,crate::windows_navidrome::Controller>,audio:tauri::State<'_,Controller>)->Result<crate::windows_navidrome::Status,String>{
 let c=audio.inner().clone();let intent=c.admit_intent();let prepared=state.prepare_account(server,username,password).await?;
 let value=blocking(move||c.account_change(crate::windows_mpd::session::AccountChange::Connect(prepared),&intent)).await?;
 if app.emit("navidrome-configuration-changed",()).is_err(){eprintln!("Windows Navidrome: notification_unavailable");}Ok(value)
}
#[tauri::command]
async fn navidrome_disconnect(app:tauri::AppHandle,audio:tauri::State<'_,Controller>)->Result<crate::windows_navidrome::Status,String>{
 let c=audio.inner().clone();let intent=c.admit_intent();let value=blocking(move||c.account_change(crate::windows_mpd::session::AccountChange::Disconnect,&intent)).await?;
 if app.emit("navidrome-configuration-changed",()).is_err(){eprintln!("Windows Navidrome: notification_unavailable");}Ok(value)
}
#[tauri::command]
async fn navidrome_albums(connection_id:String,query:String,sort:crate::windows_navidrome::Sort,offset:usize,page_size:Option<usize>,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<crate::windows_navidrome::Page,String>{state.albums(connection_id,query,sort,offset,page_size).await}
#[tauri::command]
async fn navidrome_album(connection_id:String,id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<crate::windows_navidrome::Detail,String>{state.album(connection_id,id).await}
#[tauri::command]
async fn navidrome_search(connection_id:String,query:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<crate::windows_navidrome::SearchResults,String>{state.search(connection_id,query).await}
#[tauri::command]
async fn navidrome_artist(connection_id:String,id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<crate::windows_navidrome::ArtistDetail,String>{state.artist(connection_id,id).await}
#[tauri::command]
async fn navidrome_explore(connection_id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<crate::windows_navidrome::Explore,String>{state.explore(connection_id).await}
#[tauri::command]
async fn navidrome_favorites(connection_id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<Vec<crate::windows_navidrome::api::Album>,String>{state.favorites(connection_id,None,None).await}
#[tauri::command]
async fn navidrome_set_favorite(connection_id:String,id:String,enabled:bool,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<Vec<crate::windows_navidrome::api::Album>,String>{state.favorites(connection_id,Some(id),Some(enabled)).await}
#[tauri::command]
async fn navidrome_player_navigation(connection_id:String,song_id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<crate::windows_navidrome::PlayerNavigation,String>{state.navigation(connection_id,song_id).await}
#[tauri::command]
async fn navidrome_song_navigation(locator:crate::audio::MediaLocator,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<serde_json::Value,String>{state.song_navigation(locator).await}
#[tauri::command]
async fn navidrome_song_cover_id(connection_id:String,song_id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<Option<String>,String>{state.song_cover_id(connection_id,song_id).await}
#[tauri::command]
async fn navidrome_cover(connection_id:String,id:String,state:tauri::State<'_,crate::windows_navidrome::Controller>)->Result<String,String>{state.cover(connection_id,id).await}
#[tauri::command]
async fn navidrome_play_album(connection_id:String,id:String,start_index:usize,intent:Option<String>,state:tauri::State<'_,crate::windows_navidrome::Controller>,audio:tauri::State<'_,Controller>)->Result<(),String>{
 let c=audio.inner().clone();let intent=intent.unwrap_or_else(||c.admit_intent());
 if !c.intent_current(&intent){return Err("PLAYBACK_SUPERSEDED".into());}
 let selection=state.prepared_album(&connection_id,&id,start_index)?;
 blocking(move||c.load_admitted(selection,&intent).map(|_|())).await
}

// Arbitrary renderer paths/URLs are never admitted as remote audio. Stable
// locators bind the active account, and only display metadata is accepted here.
fn locator_selection(tracks:Vec<crate::audio::TrackMetadata>,start:usize,library:&Library,navidrome:&crate::windows_navidrome::Controller,plex:&crate::windows_plex::Controller)->protocol::Result<crate::windows_mpd::library::Selection>{
 if tracks.is_empty()||tracks.len()>1000||start>=tracks.len(){return Err("invalid_argument");}
 let local_uris=tracks.iter().filter_map(|track|match &track.media_locator {Some(crate::audio::MediaLocator::Local{uri})=>Some(uri.clone()),_=>None}).collect::<Vec<_>>();
 let mut local=if local_uris.is_empty(){Vec::new()}else{library.compatibility_selection(&local_uris,0)?.tracks}.into_iter();
 let mut rows=Vec::new();
 for track in tracks {
  match track.media_locator {
   Some(crate::audio::MediaLocator::Local{..})=>rows.push(local.next().ok_or("library_changed")?),
   Some(locator@crate::audio::MediaLocator::Navidrome{..})=>{
    if [&track.title,&track.artist,&track.album].iter().any(|v|v.len()>4096||v.contains('\0')){return Err("NAVIDROME_INVALID");}
    let source=navidrome.bind_locator(&locator).map_err(|e|crate::windows_navidrome::playback_error(&e))?;
    let crate::windows_mpd::queue_v3::Source::Navidrome{song_id,..}=&source else{return Err("NAVIDROME_INVALID")};
    rows.push(Track{id:song_id.clone(),title:track.title,artist:track.artist,album:track.album,disc:0,number:0,file_name:String::new(),catalog_album_id:None,path:Default::default(),media_root:None,radio:None,upnp:None,remote_source:Some(source),remote_connection_id:None,plex_image:None,remote_lease:None});
   }
   Some(locator@crate::audio::MediaLocator::Plex{..})=>{
    if [&track.title,&track.artist,&track.album].iter().any(|v|v.len()>4096||v.contains('\0')){return Err("PLEX_INVALID");}
    let source=plex.bind_locator(&locator).map_err(|e|crate::windows_plex::playback_error(&e))?;
    let crate::windows_mpd::queue_v3::Source::Plex{rating_key,..}=&source else{return Err("PLEX_INVALID")};
    let plex_image=plex.presentation_image(&source,track.plex_image);
    rows.push(Track{id:rating_key.clone(),title:track.title,artist:track.artist,album:track.album,disc:0,number:0,file_name:String::new(),catalog_album_id:None,path:Default::default(),media_root:None,radio:None,upnp:None,remote_source:Some(source),remote_connection_id:None,plex_image,remote_lease:None});
   }
   _=>return Err("windows_feature_unavailable"),
  }
 }
 let root=rows.iter().find_map(|t|t.media_root.clone()).unwrap_or_default();
 let album_id=rows.first().and_then(|t|t.catalog_album_id.clone()).filter(|id|rows.iter().all(|t|t.catalog_album_id.as_ref()==Some(id)));
 Ok(crate::windows_mpd::library::Selection{root,tracks:rows,start,album_id})
}
#[tauri::command]
async fn windows_play_tracks(tracks:Vec<crate::audio::TrackMetadata>,start_index:usize,intent:String,library:tauri::State<'_,Library>,navidrome:tauri::State<'_,crate::windows_navidrome::Controller>,plex:tauri::State<'_,crate::windows_plex::Controller>,audio:tauri::State<'_,Controller>)->Result<(),String>{
 let library=library.inner().clone();let navidrome=navidrome.inner().clone();let plex=plex.inner().clone();let c=audio.inner().clone();
 blocking(move||{if !c.intent_current(&intent){return Err("PLAYBACK_SUPERSEDED");}c.load_admitted(locator_selection(tracks,start_index,&library,&navidrome,&plex)?,&intent).map(|_|())}).await
}
#[tauri::command]
async fn windows_enqueue_tracks(tracks:Vec<crate::audio::TrackMetadata>,placement:crate::windows_mpd::queue_edit::Placement,intent:String,library:tauri::State<'_,Library>,navidrome:tauri::State<'_,crate::windows_navidrome::Controller>,plex:tauri::State<'_,crate::windows_plex::Controller>,audio:tauri::State<'_,Controller>)->Result<usize,String>{
 let library=library.inner().clone();let navidrome=navidrome.inner().clone();let plex=plex.inner().clone();let c=audio.inner().clone();
 blocking(move||{if !c.intent_current(&intent){return Err("PLAYBACK_SUPERSEDED");}let count=tracks.len();c.enqueue(locator_selection(tracks,0,&library,&navidrome,&plex)?,placement,&intent)?;Ok(count)}).await
}

#[tauri::command]
async fn search_radio(search: crate::windows_radio::Search, state: tauri::State<'_, crate::windows_radio_host::Controller>) -> Result<crate::windows_radio::Page, String> {
    state.inner().clone().search(search).await
}
#[tauri::command]
async fn radio_filters(state: tauri::State<'_, crate::windows_radio_host::Controller>) -> Result<crate::windows_radio::Filters, String> {
    state.inner().clone().filters().await
}
#[tauri::command]
async fn radio_favorites(id: Option<String>, enabled: Option<bool>, name: Option<String>, url: Option<String>, state: tauri::State<'_, crate::windows_radio_host::Controller>) -> Result<Vec<crate::windows_radio::Station>, String> {
    state.inner().clone().favorites(id, enabled, name, url).await
}
#[tauri::command]
async fn play_radio(id: String, intent: Option<String>, radio: tauri::State<'_, crate::windows_radio_host::Controller>, state: tauri::State<'_, Controller>) -> Result<(), String> {
    if !crate::windows_radio::valid_id(&id) { return Err("RADIO_INVALID".into()); }
    let radio = radio.inner().clone(); let controller = state.inner().clone();
    let token = intent.unwrap_or_else(|| controller.admit_intent());
    let selected = radio.station(id.clone()).await?.selection().map_err(str::to_owned)?;
    blocking(move || controller.load_admitted(selected, &token).map(|_| ())).await?;
    tauri::async_runtime::spawn(async move { radio.count_click(&id).await; });
    Ok(())
}
#[tauri::command]
async fn stop_radio(id: String, intent: Option<String>, state: tauri::State<'_, Controller>) -> Result<(), String> {
    if !crate::windows_radio::valid_id(&id) { return Err("RADIO_INVALID".into()); }
    let controller = state.inner().clone(); let token = intent.unwrap_or_else(|| controller.admit_intent());
    blocking(move || controller.stop_radio(id, Some(token)).map(|_| ())).await
}

#[tauri::command]
fn windows_bootstrap() -> WindowsBootstrap {
    WindowsBootstrap {
        version: env!("CARGO_PKG_VERSION"),
        milestone: "windows-shared-ui-preview-44",
        platform: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        playback_available: true,
        runtime_version: crate::windows_mpd::session::runtime_version(),
    }
}

async fn blocking<T: Send + 'static>(
    action: impl FnOnce() -> protocol::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(action)
        .await
        .map_err(|_| "worker_failed".to_owned())?
        .map_err(str::to_owned)
}

#[tauri::command]
async fn windows_status(state: tauri::State<'_, Controller>) -> Result<Snapshot, String> {
    let controller = state.inner().clone();
    blocking(move || controller.status()).await
}

#[tauri::command]
async fn windows_open_file(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Controller>,
) -> Result<Snapshot, String> {
    let controller = state.inner().clone();
    blocking(move || {
        let selected = rfd::FileDialog::new()
            .set_parent(&window)
            .add_filter(
                "Audio",
                &[
                    "flac", "wav", "mp3", "ogg", "opus", "m4a", "aac", "aif", "aiff", "wv",
                ],
            )
            .pick_file();
        match selected {
            Some(file) => controller.load(file),
            None => controller.status(),
        }
    })
    .await
}

#[tauri::command]
async fn windows_action(
    generation: u64,
    action: Action,
    intent: Option<String>,
    state: tauri::State<'_, Controller>,
) -> Result<Snapshot, String> {
    let controller = state.inner().clone();
    blocking(move || match intent {
        Some(token) => controller.action_admitted(generation, action, &token),
        None => controller.action(generation, action),
    })
    .await
}

#[tauri::command]
async fn windows_library_status(state: tauri::State<'_, Library>) -> Result<LibraryStatus, String> {
    let library = state.inner().clone();
    blocking(move || library.status()).await
}
#[tauri::command]
async fn windows_library_choose(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Library>,
) -> Result<LibraryStatus, String> {
    let library = state.inner().clone();
    blocking(
        move || match rfd::FileDialog::new().set_parent(&window).pick_folder() {
            Some(root) => library.select(root),
            None => library.status(),
        },
    )
    .await
}
#[tauri::command]
async fn windows_library_refresh(
    state: tauri::State<'_, Library>,
) -> Result<LibraryStatus, String> {
    let library = state.inner().clone();
    blocking(move || library.refresh()).await
}
#[tauri::command]
async fn windows_check_library(
    source_id: Option<String>,
    expected_folders: Option<Vec<String>>,
    state: tauri::State<'_, Library>,
) -> Result<bool, String> {
    let library = state.inner().clone();
    blocking(move || library.check_changes(source_id.as_deref(), expected_folders.as_deref())).await
}
#[tauri::command]
async fn windows_rescan_library(state: tauri::State<'_, Library>) -> Result<(), String> {
    let library = state.inner().clone();
    blocking(move || library.rescan_confirmed()).await
}
#[tauri::command]
async fn windows_library_albums(
    query: String,
    offset: usize,
    limit: usize,
    order: Option<String>,
    state: tauri::State<'_, Library>,
) -> Result<Albums, String> {
    let library = state.inner().clone();
    blocking(move || {
        library.albums_ordered(&query, offset, limit, order.as_deref().unwrap_or("artist"))
    })
    .await
}
#[tauri::command]
async fn windows_library_tracks(
    album_id: String,
    state: tauri::State<'_, Library>,
) -> Result<Vec<Track>, String> {
    let library = state.inner().clone();
    blocking(move || library.tracks(&album_id)).await
}
#[tauri::command]
async fn windows_library_cover(
    album_id: String,
    state: tauri::State<'_, Library>,
) -> Result<Option<String>, String> {
    let library = state.inner().clone();
    blocking(move || library.cover(&album_id)).await
}
#[tauri::command]
async fn windows_favorite_cover(album_id: String, state: tauri::State<'_, Library>) -> Result<crate::windows_mpd::library::FavoriteCover, String> {
    let library = state.inner().clone();
    blocking(move || library.favorite_cover(&album_id)).await
}
#[tauri::command]
async fn windows_library_play(
    album_id: String,
    track_id: Option<String>,
    library: tauri::State<'_, Library>,
    state: tauri::State<'_, Controller>,
) -> Result<Snapshot, String> {
    let library = library.inner().clone();
    let controller = state.inner().clone();
    blocking(move || controller.load_queue(library.select_queue(&album_id, track_id.as_deref())?))
        .await
}

#[tauri::command]
async fn windows_catalog(state: tauri::State<'_, Library>) -> Result<serde_json::Value, String> {
    let library = state.inner().clone();
    blocking(move || library.compatibility_catalog()).await
}
#[tauri::command]
async fn windows_catalog_tracks(
    album_id: String,
    state: tauri::State<'_, Library>,
) -> Result<serde_json::Value, String> {
    let library = state.inner().clone();
    blocking(move || library.compatibility_tracks(&album_id)).await
}
#[tauri::command]
async fn windows_search(
    query: String,
    state: tauri::State<'_, Library>,
) -> Result<serde_json::Value, String> {
    let library = state.inner().clone();
    blocking(move || library.compatibility_search(&query)).await
}
fn ui_path(path: std::path::PathBuf) -> String {
    crate::windows_file_info::display_path(&path)
}
#[tauri::command]
async fn windows_library_root(state: tauri::State<'_, Library>) -> Result<Option<String>, String> {
    let library = state.inner().clone();
    blocking(move || Ok(library.configured_root()?.map(ui_path))).await
}
#[tauri::command]
async fn windows_library_roots(state: tauri::State<'_, Library>) -> Result<Vec<String>, String> {
    let library = state.inner().clone();
    blocking(move || Ok(library.configured_roots()?.into_iter().map(ui_path).collect())).await
}
#[tauri::command]
async fn windows_set_libraries(roots: Vec<String>, controller: tauri::State<'_, Controller>, state: tauri::State<'_, Library>) -> Result<LibraryStatus, String> {
    let library = state.inner().clone();
    controller.admit_intent();
    blocking(move || library.select_roots(roots.into_iter().map(std::path::PathBuf::from).collect())).await
}
#[tauri::command]
async fn windows_pick_directory(window: tauri::WebviewWindow) -> Result<Option<String>, String> {
    blocking(move || {
        Ok(rfd::FileDialog::new()
            .set_parent(&window)
            .pick_folder()
            .map(ui_path))
    })
    .await
}
#[tauri::command]
async fn windows_set_library(
    root: String,
    controller: tauri::State<'_, Controller>,
    state: tauri::State<'_, Library>,
) -> Result<LibraryStatus, String> {
    let library = state.inner().clone();
    controller.admit_intent();
    blocking(move || library.select(std::path::PathBuf::from(root))).await
}
#[tauri::command]
async fn windows_play_selection(
    uris: Vec<String>,
    start_index: usize,
    intent: String,
    library: tauri::State<'_, Library>,
    state: tauri::State<'_, Controller>,
) -> Result<Snapshot, String> {
    let library = library.inner().clone();
    let controller = state.inner().clone();
    blocking(move || {
        controller.load_admitted(
            library.compatibility_selection(&uris, start_index)?,
            &intent,
        )
    })
    .await
}

#[tauri::command]
async fn windows_enqueue(
    uris: Vec<String>,
    placement: crate::windows_mpd::queue_edit::Placement,
    intent: String,
    library: tauri::State<'_, Library>,
    state: tauri::State<'_, Controller>,
) -> Result<usize, String> {
    let library = library.inner().clone();
    let controller = state.inner().clone();
    blocking(move || {
        let selected = library.compatibility_selection(&uris, 0)?;
        let count = selected.tracks.len();
        let result = controller.enqueue(selected, placement, &intent)?;
        let _ = result;
        Ok(count)
    })
    .await
}
#[tauri::command]
async fn windows_edit_queue(
    revision: String,
    action: crate::windows_mpd::queue_edit::Edit,
    intent: String,
    state: tauri::State<'_, Controller>,
) -> Result<Snapshot, String> {
    let controller = state.inner().clone();
    blocking(move || controller.edit_queue(revision, action, &intent)).await
}

#[tauri::command]
fn windows_intent(
    operation: String,
    token: Option<String>,
    state: tauri::State<'_, Controller>,
) -> Result<serde_json::Value, String> {
    match operation.as_str() {
        "admit" => Ok(serde_json::json!(state.admit_intent())),
        "current" => Ok(serde_json::json!(
            state.intent_current(token.as_deref().unwrap_or(""))
        )),
        "cancel" => {
            state.cancel_intent(token.as_deref().unwrap_or(""));
            Ok(serde_json::Value::Null)
        }
        _ => Err("invalid_argument".into()),
    }
}

#[tauri::command]
fn windows_set_title(title: String, window: tauri::WebviewWindow) -> Result<(), String> {
    if title.chars().count() > 512 || title.contains(['\0', '\r', '\n']) {
        return Err("invalid_argument".into());
    }
    window
        .set_title(&title)
        .map_err(|_| "window_unavailable".into())
}

#[tauri::command]
async fn windows_directory(
    path: String,
    state: tauri::State<'_, Library>,
) -> Result<serde_json::Value, String> {
    let library = state.inner().clone();
    blocking(move || library.compatibility_directory(&path)).await
}

#[tauri::command]
async fn windows_recover(state: tauri::State<'_, Controller>) -> Result<Snapshot, String> {
    let controller = state.inner().clone();
    blocking(move || controller.recover()).await
}

#[tauri::command]
async fn windows_smart_playlists(
    operation:String,args:serde_json::Value,
    state:tauri::State<'_,crate::windows_smart_playlists::Controller>,
    library:tauri::State<'_,Library>,
    favorites:tauri::State<'_,crate::windows_favorites::Store>,
    nav:tauri::State<'_,crate::windows_navidrome::Controller>,
    plex:tauri::State<'_,crate::windows_plex::Controller>,
)->Result<serde_json::Value,String>{
    crate::windows_smart_remote::run(operation,args,state.inner().clone(),library.inner().clone(),favorites.inner().clone(),nav.inner().clone(),plex.inner().clone()).await
}

#[tauri::command]
async fn windows_playlist_store(
    operation: String,
    args: serde_json::Value,
    state: tauri::State<'_, crate::windows_playlists::Controller>,
) -> Result<serde_json::Value, String> {
    let store = state.inner().clone();
    blocking(move || store.call(&operation, args)).await
}
#[tauri::command]
async fn windows_playlist_resolve(
    playlist_id: String,
    state: tauri::State<'_, crate::windows_playlists::Controller>,
    navidrome:tauri::State<'_,crate::windows_navidrome::Controller>,plex:tauri::State<'_,crate::windows_plex::Controller>,
    library: tauri::State<'_, Library>,
) -> Result<Vec<crate::windows_playlists::Availability>, String> {
    let store = state.inner().clone();
    let library = library.inner().clone();
    let navidrome=navidrome.inner().clone();let plex=plex.inner().clone();
    blocking(move || crate::windows_playlists::resolve_all(&store.get(&playlist_id)?, &library,&navidrome,&plex)).await
}
#[tauri::command]
async fn windows_playlist_play(
    playlist_id: String,
    start_item_id: Option<String>,
    shuffle: bool,
    intent: String,
    state: tauri::State<'_, crate::windows_playlists::Controller>,
    navidrome:tauri::State<'_,crate::windows_navidrome::Controller>,plex:tauri::State<'_,crate::windows_plex::Controller>,
    library: tauri::State<'_, Library>,
    audio: tauri::State<'_, Controller>,
) -> Result<serde_json::Value, String> {
    let store = state.inner().clone();
    let library = library.inner().clone();
    let controller = audio.inner().clone();
    let navidrome=navidrome.inner().clone();let plex=plex.inner().clone();
    blocking(move || {
        if !controller.intent_current(&intent) {
            return Err("PLAYBACK_SUPERSEDED");
        }
        let playlist = store.get(&playlist_id)?;
        let availability = crate::windows_playlists::resolve_all(&playlist, &library,&navidrome,&plex)?;
        let ready: Vec<_> = availability
            .iter()
            .map(|row| row.status == "available")
            .collect();
        let plan = crate::windows_playlists::prepare(
            &playlist,
            &ready,
            start_item_id.as_deref(),
            shuffle,
            crate::windows_playlists::random_index,
        )?;
        let tracks=plan.indices.iter().map(|index|{
            let item=&playlist.items[*index];
            crate::audio::TrackMetadata {title:item.metadata.title.clone(),artist:item.metadata.artist.clone(),album:item.metadata.album.clone(),thumb:None,plex_image:None,media_locator:Some(item.media_locator.clone()),uri:String::new(),duration:item.metadata.duration}
        }).collect();
        let mut selected=locator_selection(tracks,plan.start,&library,&navidrome,&plex)?;
        // Preserve the original Local playlist presentation snapshot.
        for (track,index) in selected.tracks.iter_mut().zip(&plan.indices){let metadata=&playlist.items[*index].metadata;track.title=metadata.title.clone();track.artist=metadata.artist.clone();track.album=metadata.album.clone();}
        controller.load_admitted(selected, &intent)?;
        Ok(serde_json::json!({"skipped_count":plan.skipped}))
    })
    .await
}

#[tauri::command]
async fn windows_artwork_progress(
    operation: String,
    args: serde_json::Value,
    store: tauri::State<'_, crate::windows_artwork_progress::Controller>,
) -> Result<serde_json::Value, String> {
    let store = store.inner().clone();
    blocking(move || store.call(&operation, args)).await
}

#[tauri::command]
async fn windows_favorites(
    operation: String,
    legacy: Option<String>,
    album: Option<serde_json::Value>,
    store: tauri::State<'_, crate::windows_favorites::Store>,
    plex:tauri::State<'_,crate::windows_plex::Controller>,
) -> Result<serde_json::Value, String> {
    let store = store.inner().clone();
    let settings=plex.public_settings().await?;
    let current_server=settings.server_id;
    blocking(move || {
        let value = match operation.as_str() {
            "list" => serde_json::to_value(store.list()?.map(|rows|rows.into_iter().filter(|row|row.source!="plex"||row.plex_server_id==current_server).collect::<Vec<_>>())),
            "import" => serde_json::to_value(
                store.import(legacy.as_deref().ok_or("invalid_argument")?)?.into_iter().filter(|row|row.source!="plex"||row.plex_server_id==current_server).collect::<Vec<_>>(),
            ),
            "toggle" => {
                let mut value=album.ok_or("invalid_argument")?;
                if value.get("source").and_then(serde_json::Value::as_str)==Some("plex") {
                    value["plex_server_id"]=serde_json::to_value(current_server.as_ref().ok_or("PLEX_CONTEXT_CHANGED")?).map_err(|_|"PLEX_INVALID")?;
                    value["plex_thumb"]=value.get("plex_image").cloned().unwrap_or(serde_json::Value::Null);
                }
                let album: crate::windows_favorites::Album =
                    serde_json::from_value(value)
                        .map_err(|_| "invalid_argument")?;
                serde_json::to_value(store.toggle(album)?)
            }
            _ => return Err("invalid_argument"),
        };
        value.map_err(|_| "windows_favorites_failed")
    })
    .await
}

#[tauri::command]
async fn history_plex_navigation(id:String,history:tauri::State<'_,crate::windows_history::Controller>,plex:tauri::State<'_,crate::windows_plex::Controller>)->Result<crate::windows_plex::PlexTrackNavigation,String>{
 if id.len()!=64||!id.bytes().all(|b|b.is_ascii_hexdigit()){return Err("HISTORY_UNAVAILABLE".into());}
 let h=history.inner().clone();let wanted=id.clone();let locator=blocking(move||h.call(None,false)?.entries.into_iter().find(|entry|entry.id==wanted).and_then(|entry|entry.track.media_locator).ok_or("HISTORY_UNAVAILABLE")).await?;
 if !matches!(locator,crate::audio::MediaLocator::Plex{..}){return Err("HISTORY_UNAVAILABLE".into());}
 let h=history.inner().clone();let expected=locator.clone();plex.history_navigation(locator,async move{blocking(move||Ok(h.call(None,false)?.entries.iter().any(|entry|entry.id==id&&entry.track.media_locator.as_ref()==Some(&expected)))).await}).await
}
#[tauri::command]
async fn windows_listening_history(
    enabled: Option<bool>,
    clear: Option<bool>,
    history: tauri::State<'_, crate::windows_history::Controller>,
) -> Result<crate::windows_history::HistoryFile, String> {
    let history = history.inner().clone();
    blocking(move || history.call(enabled, clear.unwrap_or(false))).await
}

#[tauri::command]
async fn windows_observe_output(
    generation: u64,
    audio: tauri::State<'_, Controller>,
) -> Result<crate::windows_output::Observation, String> {
    let binding = audio.output_binding(generation).ok_or("not_observed")?;
    let permit = crate::windows_output::Permit::acquire()?;
    let audio = audio.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // COM may block inside a system call. Retain the permit until it really
        // finishes; a WebView timeout/close cannot start unbounded observers.
        let _permit = permit;
        crate::windows_output::observe(binding.identity.pid, || audio.output_binding_current(&binding))
            .map_err(str::to_owned)
    }).await.map_err(|_| "unavailable".to_owned())?
}

#[tauri::command]
async fn windows_player_source_info(
    media: serde_json::Value,
    library: tauri::State<'_, Library>,
    audio: tauri::State<'_, Controller>,
    plex:tauri::State<'_,crate::windows_plex::Controller>,
) -> Result<Option<serde_json::Value>, String> {
    use crate::{
        local_file_info::InspectionPermit,
        windows_source_info::Media,
    };
    if media.get("kind").and_then(serde_json::Value::as_str)==Some("plex"){
        let Some(remote)=crate::windows_source_info::RemoteMedia::decode(&media)else{return Ok(None);};
        let Some(_permit)=crate::windows_source_info::RemotePermit::acquire()else{return Ok(None);};
        let c=audio.inner().clone();let index=remote.queue_index;
        let before=blocking(move||{let before=c.status()?.remote_info_binding(index);Ok((_permit,before))}).await.ok();
        let Some((permit,Some(binding)))=before else{return Ok(None);};if !remote.matches(&binding){return Ok(None);}
        let crate::windows_mpd::queue_v3::Source::Plex{server_id,part_key,rating_key}=&binding.source else{return Ok(None);};
        let locator=crate::audio::MediaLocator::Plex{server_id:server_id.clone(),part_key:part_key.clone(),rating_key:Some(rating_key.clone()),file_path:None};
        let c=audio.inner().clone();let expected=binding.clone();
        let facts=plex.source_facts(locator,async move{blocking(move||{let _permit=permit;Ok(c.status()?.remote_info_binding(index)==Some(expected))}).await.unwrap_or(false)}).await;
        return Ok(facts.map(|info|serde_json::json!({"media":remote,"info":info})));
    }
    let Some(media) = Media::decode(&media) else {
        return Ok(None);
    };
    let Some(binding) = audio.source_binding(&media.uri, media.queue_index) else {
        return Ok(None);
    };
    let Some(expected_identity) = binding.identity else { return Ok(None); };
    let Ok(permit) = InspectionPermit::acquire() else {
        return Ok(None);
    };
    let library = library.inner().clone();
    let audio = audio.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let uri = media.uri.clone();
        crate::windows_source_info::acquire(
            media,
            || audio.source_binding_current(&binding),
            || crate::windows_file_info::inspect_bound(&library, &uri, &binding.path, &expected_identity),
        )
    })
    .await
    .map_err(|_| "source_info_unavailable".to_owned())
    .map(|frame|frame.and_then(|frame|serde_json::to_value(frame).ok()))
}

#[tauri::command]
async fn windows_inspect_file(
    path: String,
    library: tauri::State<'_, Library>,
) -> Result<crate::local_file_info::LocalFileInfo, crate::local_file_info::InfoError> {
    use crate::local_file_info::{InfoError, InspectionPermit};
    let permit = InspectionPermit::acquire()?;
    let library = library.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        crate::windows_file_info::inspect(&library, &path)
    })
    .await
    .map_err(|_| InfoError::Unavailable)?
}

#[tauri::command]
fn claim_audio_analyzer_host(
    host: String,
    session_id: String,
    window: tauri::Window,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::windows_player::State>,
) -> Result<crate::windows_player::Owner, String> {
    let owner=state
        .claim(window.label(), &host, session_id, || {
            app.get_webview_window(&host).is_some()
        })
        .map_err(str::to_owned)?;
    app.state::<audio_analyzer::Native>().claimed(owner.generation);
    Ok(owner)
}
#[tauri::command]
fn start_audio_analyzer(
    session_id: String,
    generation: u64,
    lease_id: String,
    window: tauri::Window,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::windows_player::State>,
) -> Result<(), String> {
    if app.get_webview_window(window.label()).is_none() {
        return Err("windows_player_host_invalid".into());
    }
    let status = state
        .start(window.label(), &session_id, generation, lease_id)
        .map_err(str::to_owned)?;
    // This ACK is a presentation lease + unavailable capability observation.
    // No FIFO/output/capture is created and no fabricated level frame is sent.
    app.emit_to(window.label(), crate::windows_player::STATUS_EVENT, status.clone())
        .map_err(|_| "windows_player_host_unavailable".to_owned())?;
    app.state::<audio_analyzer::Native>().start(app.clone(),status.owner)
}
#[tauri::command]
fn stop_audio_analyzer(
    session_id: String,
    generation: u64,
    lease_id: String,
    window: tauri::Window,
    app:tauri::AppHandle,
    state: tauri::State<'_, crate::windows_player::State>,
) -> Result<(), String> {
    state
        .stop(window.label(), &session_id, generation, &lease_id)
        .map_err(str::to_owned)?;
    app.state::<audio_analyzer::Native>().stopped(window.label(),&session_id,generation,&lease_id);
    Ok(())
}
#[tauri::command]
fn release_audio_analyzer_host(
    session_id: String,
    generation: u64,
    window: tauri::Window,
    app:tauri::AppHandle,
    state: tauri::State<'_, crate::windows_player::State>,
) -> Result<(), String> {
    state
        .release(window.label(), &session_id, generation)
        .map_err(str::to_owned)?;
    app.state::<audio_analyzer::Native>().released(&session_id,generation);
    Ok(())
}

#[tauri::command]
async fn windows_get_sleep_timer(
    state: tauri::State<'_, crate::windows_sleep_timer::Controller>,
) -> Result<crate::windows_sleep_timer::Snapshot, String> {
    let state = state.inner().clone();
    blocking(move || state.get()).await
}
#[tauri::command]
async fn windows_set_sleep_timer(
    state: tauri::State<'_, crate::windows_sleep_timer::Controller>,
    minutes: Option<u32>,
) -> Result<crate::windows_sleep_timer::Snapshot, String> {
    let state = state.inner().clone();
    blocking(move || state.set(minutes)).await
}

pub fn run() {
    let startup_notice = Arc::new(AtomicBool::new(false));
    let setup_notice = startup_notice.clone();
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .setup(move |app| {
            // Admit the profile before persistence, stores, catalog or engine.
            // The lock handle lives until this process terminates, even if a
            // store shutdown times out. Auxiliary windows share this process.
            let profile = app.path().app_local_data_dir()?.join("windows-profile.lock");
            if let Err(failure) = crate::windows_instance::admit(&profile) {
                eprintln!("Windows startup: {}", failure.code());
                setup_notice.store(true, Ordering::Release);
                crate::windows_instance::notify(failure);
                return Err(Box::new(failure));
            }
            app.manage(crate::windows_player::State::default());
            let program = app.path().resource_dir()?.join("runtimes/mpd/mpd.exe");
            let root = app.path().app_local_data_dir()?.join("mpd-pilot");
            let data = app.path().app_local_data_dir()?.join("user-data");
            let existing_profile=crate::windows_setup::existing_profile(app.path().app_local_data_dir()?.as_path())?;
            crate::persistence::initialize(data.clone())?;
            let library=Library::with_legacy_chain(
                app.path().app_local_data_dir()?.join("library-v3/folders.json"),
                vec![app.path().app_local_data_dir()?.join("library-v2/folders.json"),app.path().app_local_data_dir()?.join("library-v1/folder.json")],
            );
            library.prepare_first_run(existing_profile)?;
            app.manage(library);
            let navidrome=crate::windows_navidrome::Controller::new(data.join("windows-navidrome-v1.json"))?;
            app.manage(navidrome.clone());
            let plex=crate::windows_plex::Controller::new(data.join("windows-plex-account-v1.json"))?;
            app.manage(plex.clone());
            app.manage(crate::windows_radio_host::Controller::new(data.join("radio-favorites.json")));
            app.manage(crate::windows_smart_playlists::Controller::with_legacy(data.join("windows-smart-playlists-v2.json"),data.join("windows-smart-playlists-v1.json"))?);
            app.manage(crate::windows_playlists::Controller::new(
                data.join("playlists.json"),
            )?);
            app.manage(crate::windows_artwork_progress::Controller::new(
                data.join("windows-artwork-progress-v1.json"),
            )?);
            app.manage(crate::windows_favorites::Store::with_legacy(
                data.join("windows-favorites-v2.json"),data.join("windows-favorites-v1.json"),
            ));
            let handle = app.handle().clone();
            let history = crate::windows_history::Controller::with_legacy(
                data.join("windows-listening-history-v2.json"),Some(data.join("windows-listening-history-v1.json")),
                move || { if handle.emit(crate::windows_history::EVENT, ()).is_err() {
                    eprintln!("Windows history: notification_unavailable");
                } },
            )?;
            app.manage(history.clone());
            app.manage(crate::remote::State::default());
            app.manage(crate::upnp::State::default());
            app.manage(crate::windows_upnp::Lifecycle::default());
            app.manage(crate::windows_remote::Lifecycle::default());
            app.manage(crate::windows_scrobbling::State::default());
            crate::windows_scrobbling::start(app.handle());
            let scrobbling_handle=app.handle().clone();
            let scrobbling_sources=app.state::<Library>().inner().clone();
            app.manage(Controller::new_observed(
                program,
                root,
                app.path().app_local_data_dir()?.join("queue-v4/queue.json"),
                vec![app.path().app_local_data_dir()?.join("queue-v3/queue.json"), app.path().app_local_data_dir()?.join("queue-v2/queue.json"), app.path().app_local_data_dir()?.join("queue-v1/queue.json")],
                history,
                navidrome,
                plex,
                app.state::<Library>().inner().clone(),
                move |result|{
                    crate::windows_remote::publish(&scrobbling_handle,result);
                    crate::windows_upnp::publish(&scrobbling_handle,result);
                    if !crate::windows_scrobbling::is_enabled(&scrobbling_handle){return;}
                    let observation=match (result,scrobbling_sources.observed_sources()){
                        (Ok(snapshot),Some(sources))=>snapshot.scrobbling_observation(sources),
                        _=>crate::windows_scrobbling::sample(false,"",true,0.,0.,0,None,crate::windows_mpd::library::Sources{local:false,navidrome:false,plex:false}),
                    };
                    crate::windows_scrobbling::observe(&scrobbling_handle,observation);
                },
            )?);
            app.manage(audio_analyzer::Native::new(app.state::<Controller>().inner()));
            crate::windows_remote::start(app.handle());
            crate::windows_upnp::start(app.handle());
            // Optional desktop SMTC integration owns only this process's main
            // HWND. Initialization never starts MPD or reads media.
            let media_notify_handle = app.handle().clone();
            let media = app.get_webview_window("main").and_then(|window| window.hwnd().ok())
                .ok_or("windows_system_media_unavailable")
                .and_then(|hwnd| crate::windows_system_media::Controller::new(hwnd.0 as isize, app.state::<Controller>().inner().clone(), move |code| {
                    if media_notify_handle.emit(crate::windows_system_media::ERROR_EVENT, code).is_err() {
                        eprintln!("Windows system media: notification_unavailable");
                    }
                }));
            match media { Ok(media) => { app.manage(media); }, Err(code) => eprintln!("Windows system media: {code}") }
            let audio = app.state::<Controller>().inner().clone();
            let handle = app.handle().clone();
            app.manage(crate::windows_sleep_timer::Controller::new(
                move || audio.stop_for_sleep(),
                move |snapshot| {
                    if handle
                        .emit(crate::windows_sleep_timer::EVENT, snapshot)
                        .is_err()
                    {
                        eprintln!("Windows sleep timer: notification_unavailable");
                    }
                },
            )?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::windows_upnp::get_upnp_receiver,crate::windows_upnp::set_upnp_receiver,
            crate::windows_remote::get_remote_control,crate::windows_remote::set_remote_control,crate::windows_remote::pair_remote_control,crate::windows_remote::revoke_remote_control,crate::windows_remote::remote_control_qr,
            album_ab::ab_tracks,album_ab::ab_begin,album_ab::ab_load,album_ab::ab_decode,album_ab::ab_start,album_ab::ab_alive,album_ab::ab_end,album_ab::ab_cancel,
            get_local_track_tags,get_local_album_tag_inspection,get_local_album_doctor,get_online_album_metadata_details,search_online_album_metadata,
            crate::windows_scrobbling::get_scrobbling,crate::windows_scrobbling::set_scrobbling,
            crate::local_diagnostics::get_local_diagnostics,get_online_album_cover,get_online_cover_cache_status,get_edition_profile,get_edition_cache_namespace,navidrome_edition_catalog,windows_preferences,windows_sources,windows_configure_sources,windows_config_save_operation,windows_local_import_preview,windows_local_import_artwork,windows_audio_devices,windows_confirm_direct,windows_direct_mute,windows_gain,windows_play_tracks,windows_enqueue_tracks,
            crate::windows_plex::ipc::plex_create_pin,crate::windows_plex::ipc::plex_check_pin,crate::windows_plex::ipc::plex_get_servers,crate::windows_plex::ipc::get_plex_libraries,crate::windows_plex::ipc::get_plex_collections,crate::windows_plex::ipc::get_collection_albums,crate::windows_plex::ipc::get_artist_portrait,crate::windows_plex::ipc::get_artist_albums,crate::windows_plex::ipc::get_artist_top_tracks,crate::windows_plex::ipc::search_plex,crate::windows_plex::ipc::get_album_tracks,crate::windows_plex::ipc::get_plex_discover,crate::windows_plex::ipc::get_plex_discover_recent,crate::windows_plex::ipc::get_plex_discover_children,crate::windows_plex::ipc::get_plex_discover_artist,crate::windows_plex::ipc::get_plex_library_mix_years,crate::windows_plex::ipc::get_plex_library_mix_styles,crate::windows_plex::ipc::get_plex_library_mix,crate::windows_plex::ipc::get_plex_albums,crate::windows_plex::ipc::get_plex_image,crate::windows_plex::ipc::open_external_url,crate::windows_plex::ipc::resolve_player_plex_navigation,
            navidrome_configuration,navidrome_status,navidrome_connect,navidrome_disconnect,navidrome_albums,navidrome_album,navidrome_search,navidrome_artist,navidrome_explore,navidrome_favorites,navidrome_set_favorite,navidrome_player_navigation,navidrome_song_navigation,navidrome_song_cover_id,navidrome_cover,navidrome_play_album,
            windows_bootstrap,
            search_radio, radio_filters, radio_favorites, play_radio, stop_radio,
            windows_favorites,
            windows_artwork_progress,
            windows_inspect_file,
            windows_player_source_info,
            windows_observe_output,
            windows_listening_history,
            history_plex_navigation,
            claim_audio_analyzer_host,
            start_audio_analyzer,
            stop_audio_analyzer,
            release_audio_analyzer_host,
            windows_get_sleep_timer,
            windows_set_sleep_timer,
            windows_smart_playlists,
            windows_playlist_store,
            windows_playlist_resolve,
            windows_playlist_play,
            crate::windows_equalizer::get_equalizer,
            crate::windows_equalizer::edit_equalizer,
            crate::windows_equalizer::pick_equalizer_preset,
            crate::windows_equalizer::export_equalizer_preset,
            crate::windows_equalizer::apply_equalizer,
            crate::windows_equalizer::compare_equalizer,
            windows_status,
            windows_open_file,
            windows_action,
            windows_library_status,
            windows_library_choose,
            windows_library_refresh,
            windows_check_library,
            windows_rescan_library,
            windows_library_albums,
            windows_library_tracks,
            windows_library_cover,
            windows_favorite_cover,
            windows_library_play,
            windows_catalog,
            windows_catalog_tracks,
            windows_search,
            windows_library_root,
            windows_library_roots,
            windows_set_libraries,
            windows_pick_directory,
            windows_set_library,
            windows_play_selection,
            windows_enqueue,
            windows_edit_queue,
            windows_intent,
            windows_set_title,
            windows_directory,
            windows_recover
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if crate::windows_player::exits_on_close(window.label()) {
                    api.prevent_close();
                    window.app_handle().exit(0);
                }
            }
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(analyzer)=window.app_handle().try_state::<audio_analyzer::Native>(){analyzer.closed(window.label());}
                if let Err(code) = window
                    .app_handle()
                    .state::<crate::windows_player::State>()
                    .closed(window.label())
                {
                    eprintln!("Windows player window cleanup: {code}");
                }
            }
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(_) => {
            // Profile failures already presented an actionable native message.
            // Never run a partial app, reset data, or log a raw path/credential.
            eprintln!("Windows startup: initialization_failed");
            if !startup_notice.load(Ordering::Acquire) {
                crate::windows_instance::notify(crate::windows_instance::Failure::Unavailable);
            }
            return;
        }
    };
    let started = Arc::new(AtomicBool::new(false));
    let complete = Arc::new(AtomicBool::new(false));
    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
            if !complete.load(Ordering::Acquire) {
                api.prevent_exit();
                if !started.swap(true, Ordering::AcqRel) {
                    crate::windows_upnp::begin_shutdown(handle);
                    let upnp_handle=handle.clone();
                    let upnp_shutdown=tauri::async_runtime::spawn_blocking(move||crate::windows_upnp::shutdown(&upnp_handle));
                    crate::windows_remote::begin_shutdown(handle);
                    let remote_handle=handle.clone();
                    let remote_shutdown=tauri::async_runtime::spawn_blocking(move||crate::windows_remote::shutdown(&remote_handle));
                    album_ab::shutdown();
                    if let Some(analyzer)=handle.try_state::<audio_analyzer::Native>(){analyzer.shutdown();}
                    crate::windows_scrobbling::shutdown(handle);
                    if let Err(code) = handle.state::<crate::windows_player::State>().shutdown() {
                        eprintln!("Windows player shutdown: {code}");
                    }
                    let controller = handle.state::<Controller>().inner().clone();
                    handle.state::<Library>().stop();
                    let timer = handle
                        .state::<crate::windows_sleep_timer::Controller>()
                        .inner()
                        .clone();
                    timer.begin_shutdown();
                    let radio = handle.state::<crate::windows_radio_host::Controller>().inner().clone();
                    radio.begin_shutdown();
                    if let Some(media) = handle.try_state::<crate::windows_system_media::Controller>() { media.begin_shutdown(); }
                    let navidrome=handle.state::<crate::windows_navidrome::Controller>().inner().clone();
                    let plex=handle.state::<crate::windows_plex::Controller>().inner().clone();
                    controller.begin_shutdown();
                    let playlists = handle
                        .state::<crate::windows_playlists::Controller>()
                        .inner()
                        .clone();
                    playlists.begin_shutdown();
                    let smart = handle.state::<crate::windows_smart_playlists::Controller>().inner().clone();
                    smart.begin_shutdown();
                    let favorites = handle.state::<crate::windows_favorites::Store>().inner().clone();
                    favorites.begin_shutdown();
                    let artwork = handle.state::<crate::windows_artwork_progress::Controller>().inner().clone();
                    artwork.begin_shutdown();
                    let history = handle.state::<crate::windows_history::Controller>().inner().clone();
                    history.begin_shutdown();
                    let handle = handle.clone();
                    let complete = complete.clone();
                    tauri::async_runtime::spawn(async move {
                        let radio_shutdown = tauri::async_runtime::spawn_blocking(move || radio.shutdown());
                        // Independent store and audio workers drain concurrently. The
                        // store publishes success only after its atomic write finishes.
                        let media_handle = handle.clone();
                        let media_shutdown = tauri::async_runtime::spawn_blocking(move || {
                            media_handle.try_state::<crate::windows_system_media::Controller>().map(|media| media.shutdown()).unwrap_or(Ok(()))
                        });
                        let smart_shutdown = tauri::async_runtime::spawn_blocking(move || smart.shutdown());
                        let history_shutdown =
                            tauri::async_runtime::spawn_blocking(move || history.shutdown());
                        let timer_shutdown =
                            tauri::async_runtime::spawn_blocking(move || timer.shutdown());
                        let artwork_shutdown =
                            tauri::async_runtime::spawn_blocking(move || artwork.shutdown());
                        let favorites_shutdown =
                            tauri::async_runtime::spawn_blocking(move || favorites.shutdown());
                        let store_shutdown =
                            tauri::async_runtime::spawn_blocking(move || playlists.shutdown());
                        match tauri::async_runtime::spawn_blocking(move || controller.shutdown())
                            .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows MPD shutdown: {code}"),
                            Err(_) => eprintln!("Windows MPD shutdown: worker_failed"),
                        }
                        // Keep the source proxy alive through the final owned
                        // MPD checkpoint/termination; only then drain the host.
                        match tauri::async_runtime::spawn_blocking(move||navidrome.shutdown()).await {
                            Ok(Ok(()))=>{},Ok(Err(_))=>eprintln!("Windows Navidrome shutdown: unavailable"),Err(_)=>eprintln!("Windows Navidrome shutdown: worker_failed"),
                        }
                        match tauri::async_runtime::spawn_blocking(move||plex.shutdown()).await {Ok(Ok(()))=>{},_=>eprintln!("Windows Plex shutdown: PLEX_UNAVAILABLE")};
                        match radio_shutdown.await {
                            Ok(Ok(())) => {},
                            Ok(Err(code)) => eprintln!("Windows radio shutdown: {code}"),
                            Err(_) => eprintln!("Windows radio shutdown: worker_failed"),
                        }
                        match media_shutdown.await {
                            Ok(Ok(())) => {},
                            Ok(Err(code)) => eprintln!("Windows system media shutdown: {code}"),
                            Err(_) => eprintln!("Windows system media shutdown: worker_failed"),
                        }
                        match store_shutdown.await {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows playlist shutdown: {code}"),
                            Err(_) => eprintln!("Windows playlist shutdown: worker_failed"),
                        }
                        match artwork_shutdown.await {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows artwork progress shutdown: {code}"),
                            Err(_) => eprintln!("Windows artwork progress shutdown: worker_failed"),
                        }
                        match favorites_shutdown.await {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows favorites shutdown: {code}"),
                            Err(_) => eprintln!("Windows favorites shutdown: worker_failed"),
                        }
                        match smart_shutdown.await {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows smart playlists shutdown: {code}"),
                            Err(_) => eprintln!("Windows smart playlists shutdown: worker_failed"),
                        }
                        match history_shutdown.await {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows history shutdown: {code}"),
                            Err(_) => eprintln!("Windows history shutdown: worker_failed"),
                        }
                        match timer_shutdown.await {
                            Ok(Ok(())) => {}
                            Ok(Err(code)) => eprintln!("Windows timer shutdown: {code}"),
                            Err(_) => eprintln!("Windows timer shutdown: worker_failed"),
                        }
                        if upnp_shutdown.await.is_err(){eprintln!("Windows UPnP: shutdown_worker_failed");}
                        if remote_shutdown.await.is_err(){eprintln!("Windows remote: shutdown_worker_failed");}
                        // Any surviving job handles are closed by process exit;
                        // KILL_ON_JOB_CLOSE prevents an orphan engine on crash.
                        complete.store(true, Ordering::Release);
                        handle.exit(code.unwrap_or(0));
                    });
                }
            }
        }
    });
}
