//! The original Plex command DTOs, using one current account binding per read.
use super::{host::Controller,core,Client,INVALID};
use super::{PlexDiscoverPage,PlexDiscoverRecent,PlexDiscoverHub,PlexDiscoverArtistContent,PlexLibraryMixPage,PlexMixYears,PlexMixStyles,PlexStyleSelection};
impl Controller {
 async fn use_client<T,F,Fut>(&self,work:F)->Result<T,String> where F:FnOnce(Client)->Fut,Fut:std::future::Future<Output=Result<T,String>> {
  let registry=self.registry();let binding=registry.selected()?;let client=binding.client.clone();
  self.read(async{let value=work(client).await;registry.with_current(&binding,||value)}).await
 }
 pub(crate) async fn pin(&self)->Result<core::PlexPin,String>{self.read(core::request_plex_pin()).await}
 pub(crate) async fn check_pin(&self,id:u64)->Result<Option<String>,String>{if id==0||id>9_007_199_254_740_991{return Err(INVALID.into());}self.read(core::check_plex_pin(id)).await}
 pub(crate) async fn servers(&self,token:String)->Result<Vec<core::PlexServerResource>,String>{let token=if token.is_empty(){self.settings().await?.plex_token}else{token};if token.is_empty()||token.len()>4096||token.chars().any(char::is_control){return Err(INVALID.into());}self.read(core::get_plex_servers(&token)).await}
 pub(crate) async fn libraries(&self)->Result<Vec<core::catalog::PlexLibrary>,String>{self.use_client(|c|async move{c.libraries().await}).await}
 pub(crate) async fn albums(&self,section:String,sort:String)->Result<core::catalog::PlexAlbumCatalog,core::catalog::PlexAlbumCatalogError>{
  let fail=|message:String|core::catalog::PlexAlbumCatalogError{completeness:core::catalog::AlbumCatalogCompleteness::Unknown,received:0,message};let registry=self.registry();let binding=registry.selected().map_err(&fail)?;
  self.catalog_read(async{let value=binding.client.albums(&section,&sort).await;registry.with_current(&binding,||Ok(value))}).await.map_err(fail)?
 }
 pub(crate) async fn album_tracks(&self,id:String)->Result<Vec<core::content::PlexTrack>,String>{self.use_client(|c|async move{c.album_tracks(&id).await}).await}
 pub(crate) async fn search(&self,query:String,section:Option<String>)->Result<core::content::PlexSearchResults,String>{self.use_client(|c|async move{c.search(&query,section.as_deref()).await}).await}
 pub(crate) async fn collections(&self,id:String)->Result<Vec<core::content::PlexCollection>,String>{self.use_client(|c|async move{c.collections(&id).await}).await}
 pub(crate) async fn collection_albums(&self,id:String)->Result<Vec<core::catalog::PlexAlbum>,String>{self.use_client(|c|async move{c.collection_albums(&id).await}).await}
 pub(crate) async fn artist_albums(&self,id:String)->Result<Vec<core::catalog::PlexAlbum>,String>{self.use_client(|c|async move{c.artist_albums(&id).await}).await}
 pub(crate) async fn artist_tracks(&self,id:String)->Result<Vec<core::content::PlexTrack>,String>{self.use_client(|c|async move{c.artist_tracks(&id).await}).await}
 pub(crate) async fn artist_portrait(&self,server:String,id:String)->Result<Option<crate::plex::PlexImageRef>,String>{self.use_client(|c|async move{c.artist_portrait(&server,&id).await}).await}
 pub(crate) async fn image(&self,image:crate::plex::PlexImageRef)->Result<Vec<u8>,String>{let registry=self.registry();let binding=registry.selected()?;let value=self.artwork_read(&binding,binding.client.image(&image)).await;registry.with_current(&binding,||value)}
 pub(crate) async fn source_facts<F>(&self,locator:crate::audio::MediaLocator,confirm:F)->Option<crate::windows_source_info::RemoteFacts> where F:std::future::Future<Output=bool>{
  let registry=self.registry();let binding=registry.selected().ok()?;
  self.read(async{let info=binding.client.content().source_facts(&binding.stream_http,&locator).await;
   if !confirm.await{return Ok(None);}registry.with_current(&binding,||Ok(info))
  }).await.ok().flatten()
 }
 pub(crate) async fn history_navigation<F>(&self,locator:crate::audio::MediaLocator,confirm:F)->Result<core::content::PlexTrackNavigation,String> where F:std::future::Future<Output=Result<bool,String>>{
  self.use_client(|client|async move{let navigation=client.track_navigation(&locator).await?;if !confirm.await?{return Err("HISTORY_UNAVAILABLE".into());}Ok(navigation)}).await
 }
 pub(crate) async fn navigation(&self,locator:crate::audio::MediaLocator)->Result<core::content::PlexTrackNavigation,String>{self.use_client(|c|async move{c.track_navigation(&locator).await}).await}
 pub(crate) async fn discover(&self,server:String,section:String,language:String)->Result<PlexDiscoverPage,String>{self.use_client(|c|async move{c.discover(&server,&section,&language).await}).await}
 pub(crate) async fn discover_recent(&self,server:String,section:String,language:String,window:usize)->Result<PlexDiscoverRecent,String>{self.use_client(|c|async move{c.discover_recent(&server,&section,&language,window).await}).await}
 pub(crate) async fn discover_children(&self,server:String,section:String,kind:String,id:String,language:String)->Result<PlexDiscoverHub,String>{self.use_client(|c|async move{c.discover_children(&server,&section,&kind,&id,&language).await}).await}
 pub(crate) async fn discover_artist(&self,server:String,section:String,id:String,language:String)->Result<PlexDiscoverArtistContent,String>{self.use_client(|c|async move{c.discover_artist(&server,&section,&id,&language).await}).await}
 pub(crate) async fn mix_years(&self,server:String,section:String,language:String)->Result<PlexMixYears,String>{self.use_client(|c|async move{c.mix_years(&server,&section,&language).await}).await}
 pub(crate) async fn mix_styles(&self,server:String,section:String,language:String)->Result<PlexMixStyles,String>{self.use_client(|c|async move{c.mix_styles(&server,&section,&language).await}).await}
 pub(crate) async fn mix(&self,server:String,section:String,language:String,limit:usize,decade:Option<u32>,style:Option<PlexStyleSelection>)->Result<PlexLibraryMixPage,String>{self.use_client(|c|async move{c.mix(&server,&section,&language,limit,decade,style).await}).await}
}

impl Controller{
 pub(crate) async fn edition_profile(&self,server:String,library:String,album:String)->Result<Option<crate::windows_editions::policy::Profile>,String>{
  if !core::content::valid_id(&library)||!core::content::valid_id(&album){return Err("EDITIONS_SOURCE".into());}
  let registry=self.registry();let binding=registry.selected()?;
  if binding.settings.plex_server_id.as_deref()!=Some(server.as_str())||binding.client.manager.selected_server_id().as_deref()!=Some(server.as_str()){return Err("EDITIONS_SOURCE".into());}
  self.read(async{let path=format!("/library/metadata/{album}/children?X-Plex-Container-Start=0&X-Plex-Container-Size=500");
   let value=binding.client.json(&path).await?;
   registry.with_current(&binding,||{if binding.client.manager.selected_server_id().as_deref()!=Some(server.as_str()){return Err("EDITIONS_SOURCE".into());}Ok(crate::windows_editions::policy::plex_profile(&value,&library,&album))})
  }).await
 }
}
