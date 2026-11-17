//! The original UI command DTOs, with the Windows host's bounded read gate.
use super::{catalog,discovery,host::Controller};
impl Controller {
 pub(crate) async fn albums(&self,connection_id:String,query:String,sort:catalog::Sort,offset:usize,page_size:Option<usize>)->Result<catalog::Page,String>{self.read(self.registry().albums(connection_id,query,sort,offset,page_size)).await}
 pub(crate) async fn album(&self,connection_id:String,id:String)->Result<catalog::Detail,String>{self.read(self.registry().album(connection_id,id)).await}
 pub(crate) async fn search(&self,connection_id:String,query:String)->Result<discovery::SearchResults,String>{self.read(self.registry().search(connection_id,query)).await}
 pub(crate) async fn artist(&self,connection_id:String,id:String)->Result<discovery::ArtistDetail,String>{self.read(self.registry().artist_detail(connection_id,id)).await}
 pub(crate) async fn explore(&self,connection_id:String)->Result<discovery::Explore,String>{self.read(self.registry().explore(connection_id)).await}
 pub(crate) async fn navigation(&self,connection_id:String,song_id:String)->Result<catalog::PlayerNavigation,String>{self.read(self.registry().player_navigation(connection_id,song_id)).await}
 pub(crate) async fn song_navigation(&self,locator:crate::audio::MediaLocator)->Result<serde_json::Value,String>{
  let source=self.bind_locator(&locator)?;
  let crate::windows_mpd::queue_v3::Source::Navidrome{account_id,song_id}=source else{return Err(super::INVALID.into())};
  let connection=self.registry().for_account(&account_id)?;
  let value=self.navigation(connection.credentials.connection_id.clone(),song_id).await?;
  serde_json::to_value(value).map_err(|_|super::PROTOCOL.into())
 }
 pub(crate) async fn song_cover_id(&self,connection_id:String,song_id:String)->Result<Option<String>,String>{self.read(self.registry().song_cover_id(connection_id,song_id)).await}
 pub(crate) async fn cover(&self,connection_id:String,id:String)->Result<String,String>{self.read(self.registry().cover(connection_id,id)).await}
}
