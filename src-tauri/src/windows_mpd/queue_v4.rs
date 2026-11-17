//! Presentation artwork alongside the unchanged v3 logical queue. No engine,
//! HTTP, listener or account resolution is performed while decoding this file.
use super::queue_v3::{Logical,Source};
use crate::plex::PlexImageRef;
use serde::{Serialize,Deserialize};
const ERROR:&str="queue_restore_failed";
const MAX_BYTES:usize=4*1024*1024;
#[path="../windows_navidrome/unique_json.rs"]
mod unique_json;
type Result<T>=std::result::Result<T,&'static str>;
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Artwork{server_id:String,path:String}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Record{version:u8,content:Content}
#[derive(Serialize,Deserialize)]
#[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]
enum Content{Empty{},Queue{queue:Logical,artwork:Vec<Option<Artwork>>}}
// Keep presentation references scoped to the row's Plex server. Query strings,
// encoded paths, external URLs, traversal and credential-bearing references
// are never admitted. Invalid artwork is discarded, never a media failure.
pub(crate) fn valid_image(source:&Source,image:&PlexImageRef)->bool{
 let Source::Plex{server_id,..}=source else{return false;};
 if image.server_id!=*server_id || image.path.len()>4096 || crate::plex::contains_plex_token(&image.path){return false;}
 let Some(path)=image.path.strip_prefix("/library/metadata/")else{return false;};
 let mut pieces=path.split('/');let Some(id)=pieces.next()else{return false;};
 if id.is_empty()||id.len()>128||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b))||pieces.next()!=Some("thumb"){return false;}
 match pieces.next(){None=>true,Some(stamp)=>!stamp.is_empty()&&stamp.bytes().all(|b|b.is_ascii_digit())&&pieces.next().is_none()}
}
struct Writer(Vec<u8>);
impl std::io::Write for Writer{
 fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{if self.0.len().checked_add(bytes.len()).is_none_or(|n|n>MAX_BYTES){return Err(std::io::Error::other("queue_size"));}self.0.extend_from_slice(bytes);Ok(bytes.len())}
 fn flush(&mut self)->std::io::Result<()>{Ok(())}
}
pub(crate) fn encode(queue:Option<&Logical>,images:&[Option<PlexImageRef>])->Result<Vec<u8>>{
 let content=if let Some(queue)=queue{
  queue.validate()?;if images.len()!=queue.tracks.len(){return Err(ERROR);}
  let artwork=queue.tracks.iter().zip(images).map(|(row,image)|image.as_ref().filter(|image|valid_image(&row.source,image)).map(|i|Artwork{server_id:i.server_id.clone(),path:i.path.clone()})).collect();
  Content::Queue{queue:queue.clone(),artwork}
 }else{if !images.is_empty(){return Err(ERROR);}Content::Empty{}};
 let mut writer=Writer(Vec::new());serde_json::to_writer(&mut writer,&Record{version:4,content}).map_err(|_|ERROR)?;Ok(writer.0)
}
pub(crate) fn decode(bytes:&[u8])->Result<Option<(Logical,Vec<Option<PlexImageRef>>)>>{
 if bytes.len()>MAX_BYTES{return Err(ERROR);}
 let value:serde_json::Value=unique_json::readonly_unique_json(bytes).map_err(|_|ERROR)?;
 // Reuse v3's exact options/schema validation before accepting its logical
 // queue; serde PlaybackOptions otherwise allows unrecognized option names.
 let mut logical=value.clone();let object=logical.as_object_mut().ok_or(ERROR)?;object.insert("version".into(),serde_json::json!(3));
 if let Some(content)=object.get_mut("content").and_then(serde_json::Value::as_object_mut){content.remove("artwork");}
 let logical_bytes=serde_json::to_vec(&logical).map_err(|_|ERROR)?;super::queue_v3::decode(&logical_bytes)?;
 let record:Record=serde_json::from_value(value).map_err(|_|ERROR)?;if record.version!=4{return Err(ERROR);}
 match record.content{
  Content::Empty{}=>Ok(None),
  Content::Queue{queue,artwork}=>{
   queue.validate()?;if artwork.len()!=queue.tracks.len(){return Err(ERROR);}
   let images=queue.tracks.iter().zip(artwork).map(|(row,image)|image.map(|i|PlexImageRef{server_id:i.server_id,path:i.path}).filter(|image|valid_image(&row.source,image))).collect();
   Ok(Some((queue,images)))
  }
 }
}
