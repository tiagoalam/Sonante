//! Windows Plex core only. No IPC/source capability is exposed before the
//! original UI, durable remote queue and MPD effects can use it together.
mod core;
use std::sync::Arc;
use core::PlexConnectionManager;
const INVALID:&str="PLEX_INVALID";
const CHANGED:&str="PLEX_CONTEXT_CHANGED";
const UNAVAILABLE:&str="PLEX_UNAVAILABLE";
/// A projection of the Plex connection fields, not a second AppConfig/profile.
/// It is deliberately not Serialize: the token belongs in the Windows vault.
#[derive(Clone,Default)]
pub(crate) struct Settings {
    pub(crate) enabled:bool,
    pub(crate) plex_url:String,
    pub(crate) plex_token:String,
    pub(crate) plex_server_id:Option<String>,
    pub(crate) plex_server_name:Option<String>,
}
impl Settings {
    fn validate(&self)->Result<(),String>{
        if self.plex_token.len()>4096 || self.plex_token.chars().any(char::is_control)
            || self.plex_server_name.as_ref().is_some_and(|s|s.len()>2048 || s.chars().any(char::is_control))
            || self.plex_server_id.as_ref().is_some_and(|s|core::validate_plex_server_id(s).is_err()) {
            return Err(INVALID.into());
        }
        if !self.plex_url.is_empty(){
            if self.plex_url.len()>2048 || self.plex_url.chars().any(char::is_control) || self.plex_url.contains('\\'){return Err(INVALID.into());}
            let url=reqwest::Url::parse(self.plex_url.trim()).map_err(|_|INVALID)?;
            if !["http","https"].contains(&url.scheme())||url.host_str().is_none()||!url.username().is_empty()||url.password().is_some()||url.query().is_some()||url.fragment().is_some(){return Err(INVALID.into());}
        }
        if self.enabled && (self.plex_url.trim().is_empty()||self.plex_token.is_empty()){return Err(INVALID.into());}
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct Client {
    manager:Arc<PlexConnectionManager>,
    network:Arc<tokio::sync::Semaphore>,
    artwork:Arc<tokio::sync::Semaphore>,
    stop:tokio::sync::watch::Sender<bool>,
}
impl Client {
    pub(crate) fn new(settings:&Settings)->Result<Self,String>{
        settings.validate()?;
        let (stop,_)=tokio::sync::watch::channel(false);
        Ok(Self{manager:Arc::new(PlexConnectionManager::from_config(settings).map_err(|error|error.public_message())?),network:Arc::new(tokio::sync::Semaphore::new(4)),artwork:Arc::new(tokio::sync::Semaphore::new(2)),stop})
    }
    pub(crate) fn update(&self,settings:&Settings)->Result<(),String>{
        settings.validate()?;
        if *self.stop.borrow(){return Err(CHANGED.into());}
        self.manager.update_config(settings);Ok(())
    }
    async fn read<T>(&self,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{
        self.read_pool(&self.network,work).await
    }
    async fn read_pool<T>(&self,pool:&Arc<tokio::sync::Semaphore>,work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String>{
        let _slot=pool.clone().try_acquire_owned().map_err(|_|UNAVAILABLE)?;
        let mut stop=self.stop.subscribe();
        if *stop.borrow(){return Err(CHANGED.into());}
        tokio::select!{
            biased;
            _=stop.wait_for(|stopped|*stopped)=>Err(CHANGED.into()),
            result=tokio::time::timeout(std::time::Duration::from_secs(14),work)=>result.map_err(|_|UNAVAILABLE)?
        }
    }
    /// Only backend-created PMS paths reach this internal method; no command
    /// exposes arbitrary paths or an authenticated URL to the renderer.
    pub(crate) async fn json(&self,path:&str)->Result<serde_json::Value,String>{
        if !path.starts_with('/')||path.starts_with("//")||path.len()>4096||path.contains(['\0','\r','\n','\\','#'])||path.contains("://")||core::contains_plex_token(path){return Err(INVALID.into());}
        self.read(async {
            let (value,connection)=self.manager.request_json(path,"ler a biblioteca").await.map_err(|e|e.public_message())?;
            self.manager.validate_source_connection(&connection).map_err(|e|e.public_message())?;
            Ok(value)
        }).await
    }
    pub(crate) async fn image(&self,image:&crate::plex::PlexImageRef)->Result<Vec<u8>,String>{
        self.read_pool(&self.artwork,async {
            let generation=self.manager.snapshot().map_err(|e|e.public_message())?.generation;
            let bytes=self.manager.request_image(image).await.map_err(|e|e.public_message())?;
            if self.manager.snapshot().map_err(|e|e.public_message())?.generation!=generation{return Err(CHANGED.into());}
            Ok(bytes)
        }).await
    }
    pub(crate) fn shutdown(&self){self.stop.send_replace(true);self.network.close();self.artwork.close();}
}

mod store;

impl Client {
    pub(crate) async fn libraries(&self)->Result<Vec<core::catalog::PlexLibrary>,String>{self.read(core::catalog::get_music_libraries(self)).await}
    pub(crate) async fn albums(&self,section:&str,sort:&str)->Result<core::catalog::PlexAlbumCatalog,core::catalog::PlexAlbumCatalogError>{core::catalog::albums(self,section,sort).await}
}

impl Client {
    fn content(&self)->core::content::PlexClient { core::content::PlexClient::from_manager(self.manager.clone()) }
    pub(crate) async fn album_tracks(&self,id:&str)->Result<Vec<core::content::PlexTrack>,String>{
        if !core::content::valid_id(id){return Err(INVALID.into());}
        self.read(self.content().get_album_tracks(id)).await
    }
    pub(crate) async fn search(&self,query:&str,section:Option<&str>)->Result<core::content::PlexSearchResults,String>{
        if query.len()>512 || query.chars().any(char::is_control) || section.is_some_and(|s|!core::content::valid_id(s)){return Err(INVALID.into());}
        self.read(self.content().search(query,section)).await
    }
    pub(crate) async fn collections(&self,id:&str)->Result<Vec<core::content::PlexCollection>,String>{if !core::content::valid_id(id){return Err(INVALID.into());}self.read(self.content().get_collections(id)).await}
    pub(crate) async fn collection_albums(&self,id:&str)->Result<Vec<core::catalog::PlexAlbum>,String>{if !core::content::valid_id(id){return Err(INVALID.into());}self.read(self.content().get_collection_albums(id)).await}
    pub(crate) async fn artist_albums(&self,id:&str)->Result<Vec<core::catalog::PlexAlbum>,String>{if !core::content::valid_id(id){return Err(INVALID.into());}self.read(self.content().get_artist_albums(id)).await}
    pub(crate) async fn artist_tracks(&self,id:&str)->Result<Vec<core::content::PlexTrack>,String>{if !core::content::valid_id(id){return Err(INVALID.into());}self.read(self.content().get_artist_top_tracks(id)).await}
    pub(crate) async fn artist_portrait(&self,server:&str,id:&str)->Result<Option<crate::plex::PlexImageRef>,String>{self.read(self.content().get_artist_portrait(server,id)).await}
    pub(crate) async fn track_navigation(&self,locator:&crate::audio::MediaLocator)->Result<core::content::PlexTrackNavigation,String>{self.read(self.content().resolve_track_navigation(locator)).await}
}

pub(crate) use core::shared_discovery::discover::{PlexDiscoverPage,PlexDiscoverRecent,PlexDiscoverHub,PlexDiscoverArtistContent,PlexLibraryMixPage,PlexMixYears,PlexMixStyles,PlexStyleSelection};
impl Client {
 pub(crate) async fn discover(&self,server:&str,section:&str,language:&str)->Result<PlexDiscoverPage,String>{self.read(self.content().get_discover(server,section,language)).await}
 pub(crate) async fn discover_recent(&self,server:&str,section:&str,language:&str,window:usize)->Result<PlexDiscoverRecent,String>{self.read(self.content().get_discover_recent(server,section,language,window)).await}
 pub(crate) async fn discover_children(&self,server:&str,section:&str,kind:&str,id:&str,language:&str)->Result<PlexDiscoverHub,String>{self.read(self.content().get_discover_children(server,section,kind,id,language)).await}
 pub(crate) async fn discover_artist(&self,server:&str,section:&str,id:&str,language:&str)->Result<PlexDiscoverArtistContent,String>{self.read(self.content().get_discover_artist(server,section,id,language)).await}
 pub(crate) async fn mix_years(&self,server:&str,section:&str,language:&str)->Result<PlexMixYears,String>{self.read(self.content().get_library_mix_years(server,section,language)).await}
 pub(crate) async fn mix_styles(&self,server:&str,section:&str,language:&str)->Result<PlexMixStyles,String>{self.read(self.content().get_library_mix_styles(server,section,language)).await}
 pub(crate) async fn mix(&self,server:&str,section:&str,language:&str,limit:usize,decade:Option<u32>,style:Option<PlexStyleSelection>)->Result<PlexLibraryMixPage,String>{
  let content=self.content();
  self.read(async {match (decade,style) {
   (None,None)=>content.get_library_mix(server,section,language,limit).await,
   (Some(decade),None)=>content.get_library_decade_mix(server,section,language,limit,decade).await,
   (None,Some(style))=>content.get_library_style_mix(server,section,language,limit,&style).await,
   _=>Err(INVALID.into()),
  }}).await
 }
}

mod host;
pub(crate) use host::{Controller,PreparedAccount};

impl Client {
 pub(super) async fn inspect_reference(&self,source:&crate::windows_mpd::queue_v3::Source)->Result<core::content::VerifiedMedia,String>{self.read(self.content().inspect_reference(source)).await}
 pub(super) async fn verify_reference(&self,source:&crate::windows_mpd::queue_v3::Source)->Result<Option<bool>,String>{self.read(self.content().verify_reference(source)).await}
 pub(super) async fn media(&self,http:&reqwest::Client,source:&crate::windows_mpd::queue_v3::Source,range:Option<&str>)->Result<reqwest::Response,String>{
  let crate::windows_mpd::queue_v3::Source::Plex{server_id,rating_key,part_key}=source else{return Err(INVALID.into())};core::content::validate_reference(server_id,rating_key,part_key)?;
  self.manager.request_media(http,server_id,part_key,range).await
 }
}
mod proxy;

mod playback;
pub(crate) fn playback_error(error:&str)->&'static str{match error{INVALID=>INVALID,CHANGED=>CHANGED,"PLEX_STORAGE"=>"PLEX_STORAGE","PLEX_PROTOCOL"=>"PLEX_PROTOCOL","PLEX_MEDIA_MISSING"=>"PLEX_MEDIA_MISSING",_=>UNAVAILABLE}}

mod facade;
mod account_ui;
pub(crate) fn validate_favorite(album:&crate::windows_favorites::Album)->Result<(),&'static str>{
 let server=album.plex_server_id.as_deref().ok_or(INVALID)?;
 core::validate_plex_server_id(server).map_err(|_|INVALID)?;
 if !core::content::valid_id(&album.path_or_key){return Err(INVALID);}
 if album.id!=album.path_or_key||[&album.title,&album.artist].iter().any(|s|s.len()>16384||s.contains('\0'))||album.year.as_ref().is_some_and(|s|s.len()>64||s.contains('\0')){return Err(INVALID);}
 if let Some(image)=&album.plex_thumb{if image.server_id!=server{return Err(INVALID);}core::validate_favorite_image(&image.path).map_err(|_|INVALID)?;}
 Ok(())
}
pub(crate) fn validate_source(source:&crate::windows_mpd::queue_v3::Source)->Result<(),&'static str>{
 let crate::windows_mpd::queue_v3::Source::Plex{server_id,rating_key,part_key}=source else{return Err(INVALID)};
 core::content::validate_reference(server_id,rating_key,part_key).map_err(|_|INVALID)
}
pub(crate) use account_ui::{AccountDraft,PublicSettings};
#[cfg(windows)]
pub(crate) mod ipc;

pub(crate) use core::content::PlexTrackNavigation;

mod smart;

mod ab;
