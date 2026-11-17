use super::*;
fn client()->PlexClient {
 let settings=super::super::super::Settings::default();
 PlexClient::from_manager(Arc::new(PlexConnectionManager::from_config(&settings).unwrap()))
}
fn connection()->ResolvedPlexConnection{ResolvedPlexConnection{server_id:"fixture-server".into(),base_url:"http://fixture.invalid".into(),token:"FIXTURE_SECRET".into(),generation:0}}
fn item()->serde_json::Value{serde_json::json!({"type":"track","ratingKey":"track-1","parentRatingKey":"album-2","grandparentRatingKey":"artist-3","title":"Fixture song","parentTitle":"Fixture album","grandparentTitle":"Fixture artist","index":2,"duration":123456,"Media":[{"Part":[{"key":"/library/parts/1/file.flac","file":"/private/server/path.flac"}]}]})}
#[test]
fn original_track_metadata_uses_stable_http_reference_and_never_publishes_server_paths_or_tokens(){
 let track=client().parse_track_item(&connection(),&item()).unwrap();assert_eq!(track.rating_key,"track-1");assert_eq!(track.album_rating_key.as_deref(),Some("album-2"));assert_eq!(track.duration_ms,123456);
 assert_eq!(track.media_locator,MediaLocator::Plex{server_id:"fixture-server".into(),rating_key:Some("track-1".into()),part_key:"/library/parts/1/file.flac".into(),file_path:None});
 let json=serde_json::to_string(&track).unwrap();for denied in ["FIXTURE_SECRET","fixture.invalid","private/server"]{assert!(!json.contains(denied));}assert!(track.audio_info.is_none());
}
#[test]
fn unsupported_part_queries_and_unsafe_ids_are_refused_without_silently_rewriting_the_reference(){
 for path in ["http://foreign.invalid/file.flac","//foreign/file.flac","/library/parts/1/file.flac?download=1","/library/parts/1/file.flac#fragment","/library/parts/../file.flac","/library/parts/%2e%2e/file.flac","/library/parts/1/%2e%2e%2ffile.flac","/library/parts/1/file.flac?X-Plex-Token=secret"]{
  let mut value=item();value["Media"][0]["Part"][0]["key"]=serde_json::json!(path);assert!(client().parse_track_item(&connection(),&value).is_none(),"{path}");
 }
}
#[test]
fn original_exact_navigation_requires_unique_part_and_safe_album_artist_identity(){
 let c=connection();let mut value=serde_json::json!({"MediaContainer":{"Metadata":[item()]}});
 let good=parse_track_navigation(&value,"track-1","/library/parts/1/file.flac",&c).unwrap();assert_eq!(good.album.rating_key,"album-2");assert_eq!(good.artist_rating_key,"artist-3");
 value["MediaContainer"]["Metadata"][0]["Media"][0]["Part"]=serde_json::json!([{"key":"/library/parts/1/file.flac"},{"key":"/library/parts/1/file.flac"}]);assert!(parse_track_navigation(&value,"track-1","/library/parts/1/file.flac",&c).is_err());
 value["MediaContainer"]["Metadata"][0]=item();value["MediaContainer"]["Metadata"][0]["grandparentRatingKey"]=serde_json::json!("album-2");assert!(parse_track_navigation(&value,"track-1","/library/parts/1/file.flac",&c).is_err());
}
fn album_fixture(json:serde_json::Value)->Result<Vec<PlexTrack>,String>{
 use std::io::{Read,Write};
 let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
 let worker=std::thread::spawn(move||{let mut paths=Vec::new();for response in [serde_json::json!({"MediaContainer":{"machineIdentifier":"fixture-server"}}),serde_json::json!({"MediaContainer":{"Directory":[]}}),json]{let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();let mut request=Vec::new();loop{let mut chunk=[0;1024];let n=socket.read(&mut chunk).unwrap();if n==0{break;}request.extend_from_slice(&chunk[..n]);if request.windows(4).any(|v|v==b"\r\n\r\n"){break;}assert!(request.len()<16384);}let line=String::from_utf8(request).unwrap();paths.push(line.lines().next().unwrap().split('?').next().unwrap().to_owned());let body=response.to_string();write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();}paths});
 let settings=super::super::super::Settings{enabled:true,plex_url:format!("http://{address}"),plex_token:"FIXTURE_SECRET".into(),plex_server_id:Some("fixture-server".into()),plex_server_name:None};
 let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(3)).build().unwrap();let manager=PlexConnectionManager::with_clients(&settings,http.clone(),http,"http://127.0.0.1:9/unused".into());let client=PlexClient::from_manager(Arc::new(manager));
 let result=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(client.get_album_tracks("album-2"));let paths=worker.join().unwrap();assert_eq!(paths.len(),3);assert!(paths[0].starts_with("GET /identity"));assert!(paths[1].starts_with("GET /library/sections"));assert!(paths[2].starts_with("GET /library/metadata/album-2/children"));result
}
#[test]
fn actual_http_album_details_accept_complete_inventory_and_refuse_truncated_or_unsupported_rows(){
 let mut good=serde_json::json!({"MediaContainer":{"offset":0,"size":1,"totalSize":1,"Metadata":[item()]}});let result=album_fixture(good.clone()).unwrap();assert_eq!(result.len(),1);assert_eq!(result[0].track_index,2);
 good["MediaContainer"]["totalSize"]=serde_json::json!(2);assert_eq!(album_fixture(good.clone()).unwrap_err(),"PLEX_DETAILS_INCOMPLETE");
 good["MediaContainer"]["totalSize"]=serde_json::json!(1);good["MediaContainer"]["Metadata"][0]["Media"][0]["Part"][0]["key"]=serde_json::json!("/library/parts/1/file.flac?download=1");assert_eq!(album_fixture(good).unwrap_err(),"PLEX_UNSUPPORTED_MEDIA_REFERENCE");
}
