//! Nonsecret settings projection and explicit credential edits for the shared UI.
use super::{Controller,Settings,PreparedAccount,INVALID};
#[derive(serde::Serialize)]
pub(crate) struct PublicSettings {
 pub(crate) url:String,
 pub(crate) server_id:Option<String>,
 pub(crate) server_name:Option<String>,
 pub(crate) saved:bool,
 pub(crate) revision:Option<String>,
}
#[derive(serde::Deserialize)]
#[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]
enum Credential {Saved,New{token:String},Clear}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountDraft {
 url:String,server_id:Option<String>,server_name:Option<String>,credential:Credential,
}
impl Controller {
 pub(crate) async fn public_settings(&self)->Result<PublicSettings,String>{
  let s=self.settings().await?;
  Ok(PublicSettings{revision:self.registry().selected().ok().map(|b|b.generation.clone()),saved:!s.plex_token.is_empty(),url:s.plex_url,server_id:s.plex_server_id,server_name:s.plex_server_name})
 }
 pub(crate) async fn prepare_draft(&self,enabled:bool,draft:AccountDraft)->Result<PreparedAccount,String>{
  let token=match draft.credential {
   Credential::New{token}=>{if token.is_empty(){return Err(INVALID.into());}token},
   Credential::Saved=>{let token=self.settings().await?.plex_token;if token.is_empty(){return Err(INVALID.into());}token},
   Credential::Clear=>{if !draft.url.is_empty()||draft.server_id.is_some()||draft.server_name.is_some(){return Err(INVALID.into());}String::new()},
  };
  self.prepare_account(Settings{enabled,plex_url:draft.url,plex_token:token,plex_server_id:draft.server_id,plex_server_name:draft.server_name}).await
 }
}
