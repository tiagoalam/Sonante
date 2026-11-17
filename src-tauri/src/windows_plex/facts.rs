//! Same source parser as Linux; optional read-only labels, never output proof.
use super::*;
use crate::windows_source_info::RemoteFacts as SourceAudioFacts;
use serde_json::Value;
fn uint(value: &Value, max: u64) -> Option<u64> {
    let n = value.as_u64().or_else(|| { let s = value.as_str()?;
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { return None; } s.parse().ok() })?;
    (n > 0 && n <= max).then_some(n)
}
fn codec(value: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match value {
        "flac" => Some(("FLAC", "lossless", "integer")),
        "alac" => Some(("ALAC", "lossless", "integer")),
        "mp3" => Some(("MP3", "lossy", "not_applicable")),
        "aac" => Some(("AAC", "lossy", "not_applicable")),
        "vorbis" => Some(("Vorbis", "lossy", "not_applicable")),
        "opus" => Some(("Opus", "lossy", "not_applicable")),
        // PMS codec alone cannot prove WavPack lossless vs hybrid or integer vs float.
        "wavpack" => Some(("WavPack", "unknown", "not_applicable")),
        "mpc" | "musepack" => Some(("Musepack", "lossy", "not_applicable")),
        "pcm_s16le" | "pcm_s16be" | "pcm_s24le" | "pcm_s24be" | "pcm_s32le" | "pcm_s32be" => Some(("PCM", "uncompressed", "integer")),
        "pcm_f32le" | "pcm_f32be" | "pcm_f64le" | "pcm_f64be" => Some(("PCM float", "uncompressed", "float")),
        "dsd_lsbf" | "dsd_msbf" | "dsd_lsbf_planar" | "dsd_msbf_planar" => Some(("DSD", "lossless", "dsd")),
        _ => None,
    }
}
pub(super) fn parse(json: &Value, rating: &str, part_key: &str) -> Option<SourceAudioFacts> {
    let metadata = json.get("MediaContainer")?.get("Metadata")?.as_array()?;
    if metadata.len() > 4 { return None; }
    let tracks: Vec<_> = metadata.iter().filter(|t| t["ratingKey"].as_str() == Some(rating)).collect();
    if tracks.len() != 1 || tracks[0]["type"].as_str() != Some("track") { return None; }
    parse_item(tracks[0], rating, part_key)
}
fn parse_item(track: &Value, rating: &str, part_key: &str) -> Option<SourceAudioFacts> {
    if track["ratingKey"].as_str() != Some(rating) || track["type"].as_str() != Some("track") { return None; }
    let medias = track.get("Media")?.as_array()?;
    if medias.len() > 16 { return None; }
    let mut found = None;
    for media in medias {
        let parts = media.get("Part")?.as_array()?;
        if parts.len() > 16 { return None; }
        for part in parts {
            if part["key"].as_str() != Some(part_key) { continue; }
            if found.is_some() { return None; }
            found = Some((media, part));
        }
    }
    let (media, part) = found?;
    // The playback reference selects a Part, not an audio stream. Multiple audio
    // streams are ambiguous even if PMS marks a default/selected stream.
    let streams = part.get("Stream").and_then(Value::as_array);
    if streams.is_some_and(|s| s.len() > 32) { return None; }
    if streams.is_some_and(|ss| ss.iter().any(|s| uint(&s["streamType"], 4).is_none())) { return None; }
    let audio: Vec<_> = streams.into_iter().flatten().filter(|s| uint(&s["streamType"], 4) == Some(2)).collect();
    if audio.len() > 1 { return None; }
    let stream = audio.first().copied();
    let media_codec = media.get("audioCodec").and_then(Value::as_str);
    let stream_codec = stream.and_then(|s| s.get("codec")).and_then(Value::as_str);
    if media_codec.zip(stream_codec).is_some_and(|(a,b)| a != b) { return None; }
    if media.get("audioCodec").is_some_and(|v| !v.is_string()) || stream.is_some_and(|s| s.get("codec").is_some_and(|v| !v.is_string())) { return None; }
    let raw_codec = stream_codec.or(media_codec)?;
    let (codec, compression, sample_type) = codec(raw_codec)?;
    let sample_rate = stream.and_then(|s| uint(&s["samplingRate"], 50_000_000)).map(|n| n as u32);
    // FFmpeg's DSD demuxers expose the packed byte rate (native bit rate / 8).
    // Normalize only the three supported rates of an explicit DSD source codec.
    // Never reinterpret a PCM/DoP output rate or infer from file extensions.
    // https://ffmpeg.org/doxygen/8.0/dsfdec_8c_source.html (read_header)
    let sample_rate = if sample_type == "dsd" { sample_rate.map(|rate| match rate {
        352_800 => 2_822_400, 705_600 => 5_644_800, 1_411_200 => 11_289_600,
        _ => rate,
    }) } else { sample_rate };
    let mut bit_depth = stream.and_then(|s| uint(&s["bitDepth"], 64)).map(|n| n as u8);
    // Contradictory PCM codec width cannot establish an integer source depth.
    if raw_codec.starts_with("pcm_") {
        let width = raw_codec.get(5..7).and_then(|s| s.parse::<u8>().ok());
        if bit_depth.zip(width).is_some_and(|(a,b)| a != b) { bit_depth = None; }
    }
    let dsd_level = if sample_type == "dsd" { match sample_rate { Some(2_822_400) => Some(64), Some(5_644_800) => Some(128), Some(11_289_600) => Some(256), _ => None } } else { None };
    Some(SourceAudioFacts { provenance: "plex", codec, compression, sample_type,
        sample_rate, bit_depth, dsd_level, revision: None })
}


// Optional display data from an already-loaded track response. No network,
// streaming or file-path inference; only fixed labels and bounded numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlexFileInfo {
    pub container: Option<String>,
    pub codec: Option<String>,
    pub compression: Option<String>,
    pub sample_type: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub dsd_level: Option<u16>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
}
pub(super) fn track_file_info(track: &Value, rating: &str, part_key: &str) -> Option<PlexFileInfo> {
    if track["ratingKey"].as_str() != Some(rating) || track["type"].as_str() != Some("track") { return None; }
    let medias = track.get("Media")?.as_array()?;
    if medias.len() > 16 { return None; }
    let mut chosen = None;
    for media in medias {
        let parts = media.get("Part")?.as_array()?;
        if parts.len() > 16 { return None; }
        for part in parts.iter().filter(|p| p["key"].as_str() == Some(part_key)) {
            if chosen.is_some() { return None; }
            chosen = Some((media, part));
        }
    }
    let (media, part) = chosen?;
    let streams = part.get("Stream").and_then(Value::as_array);
    if streams.is_some_and(|v| v.len() > 32 || v.iter().any(|s| uint(&s["streamType"], 4).is_none())) { return None; }
    let audio: Vec<_> = streams.into_iter().flatten().filter(|s| uint(&s["streamType"], 4) == Some(2)).collect();
    if audio.len() > 1 { return None; }
    let stream = audio.first().copied();
    let facts = parse_item(track, rating, part_key);
    let container = part.get("container").or_else(|| media.get("container")).and_then(Value::as_str).and_then(|s| match s {
        "flac" => Some("FLAC"), "wav" => Some("WAV"), "aiff" => Some("AIFF"), "mp3" => Some("MP3"),
        "mp4" | "m4a" => Some("MP4/M4A"), "ogg" => Some("Ogg"), "opus" => Some("Opus"),
        "dsf" => Some("DSF"), "dff" => Some("DSDIFF"), "wv" => Some("WavPack"), "mpc" => Some("Musepack"), _ => None,
    }).map(str::to_owned);
    let stream_channels = stream.and_then(|s| uint(&s["channels"],64));
    let media_channels = uint(&media["audioChannels"],64);
    let channels = if stream_channels.zip(media_channels).is_some_and(|(a,b)|a!=b) { None } else { stream_channels.or(media_channels).map(|v|v as u8) };
    let bitrate_kbps = stream.and_then(|s| uint(&s["bitrate"],1_000_000)).or_else(||uint(&media["bitrate"],1_000_000)).map(|v|v as u32);
    if facts.is_none() && container.is_none() && channels.is_none() && bitrate_kbps.is_none() { return None; }
    Some(PlexFileInfo {
        container, codec:facts.as_ref().map(|v|v.codec.to_owned()), compression:facts.as_ref().map(|v|v.compression.to_owned()),
        sample_type:facts.as_ref().map(|v|v.sample_type.to_owned()), sample_rate:facts.as_ref().and_then(|v|v.sample_rate),
        bit_depth:facts.as_ref().and_then(|v|v.bit_depth), dsd_level:facts.as_ref().and_then(|v|v.dsd_level), channels,bitrate_kbps,
    })
}


impl PlexClient {
 pub(crate) async fn source_facts(&self,http:&reqwest::Client,locator:&MediaLocator)->Option<SourceAudioFacts>{
  let MediaLocator::Plex{server_id,part_key,rating_key:Some(rating),..}=locator else{return None;};
  validate_reference(server_id,rating,part_key).ok()?;
  let before=self.connection_manager.snapshot().ok()?;
  if before.identity.as_ref()?.machine_identifier!=*server_id{return None;}
  let base=before.current_base_url.as_ref()?;let token=before.resolved_token.as_ref().unwrap_or(&before.account_token);
  let mut url=reqwest::Url::parse(base).ok()?;
  if !matches!(url.scheme(),"http"|"https")||url.host_str().is_none()||!url.username().is_empty()||url.password().is_some()||url.query().is_some()||url.fragment().is_some(){return None;}
  url.set_path(&format!("/library/metadata/{rating}"));
  let read=async{
   let mut response=http.get(url).header("X-Plex-Token",token).header("Accept","application/json").header("X-Plex-Client-Identifier","sonante-audio-player").send().await.ok()?;
   if !response.status().is_success()||response.content_length().is_some_and(|n|n>256*1024){return None;}
   let mut body=Vec::new();while let Some(chunk)=response.chunk().await.ok()?{if body.len().saturating_add(chunk.len())>256*1024{return None;}body.extend_from_slice(&chunk);}
   let json:Value=serde_json::from_slice(&body).ok()?;
   let after=self.connection_manager.snapshot().ok()?;
   if after.generation!=before.generation||after.identity.as_ref()?.machine_identifier!=*server_id{return None;}
   if json["MediaContainer"].get("machineIdentifier").is_some_and(|id|id.as_str()!=Some(server_id)){return None;}
   parse(&json,rating,part_key)
  };
  tokio::time::timeout(Duration::from_secs(4),read).await.ok().flatten()
 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value { json!({"MediaContainer":{"Metadata":[{"type":"track","ratingKey":"42","Media":[
        {"audioCodec":"mp3","Part":[{"key":"/other","Stream":[{"streamType":2,"codec":"mp3","samplingRate":44100}]}]},
        {"audioCodec":"flac","Part":[{"key":"/chosen","Stream":[{"streamType":2,"codec":"flac","samplingRate":"96000","bitDepth":"24"}]}]}
    ]}]}}) }
    #[test]
    fn source_info_dsd_byte_rates_normalize_only_explicit_source_dsd() {
        // FFmpeg DSF/DFF demuxers report packed DSD byte rate (native bit rate / 8).
        // This fixture tests the metadata representation, not a captured PMS response.
        for codec in ["dsd_lsbf", "dsd_msbf", "dsd_lsbf_planar", "dsd_msbf_planar"] {
            for (rate, native, level) in [(352800,2822400,64), (705600,5644800,128), (1411200,11289600,256),
                (2822400,2822400,64), (5644800,5644800,128), (11289600,11289600,256)] {
                for raw in [json!(rate), json!(rate.to_string())] {
                    let mut j = fixture();
                    let media = &mut j["MediaContainer"]["Metadata"][0]["Media"][1];
                    media["audioCodec"] = json!(codec);
                    media["Part"][0]["Stream"][0]["codec"] = json!(codec);
                    media["Part"][0]["Stream"][0]["samplingRate"] = raw;
                    let info = parse(&j,"42","/chosen").unwrap();
                    assert_eq!(info.codec,"DSD"); assert_eq!(info.sample_type,"dsd");
                    assert_eq!(info.sample_rate,Some(native)); assert_eq!(info.dsd_level,Some(level));
                    if codec == "dsd_lsbf_planar" && rate < 2_000_000 {
                        println!("fixture_plex_dsd_fact={}",serde_json::to_string(&info).unwrap());
                    }
                }
            }
        }
        for raw in [json!(44100),json!(176400),json!(192000),json!(352801),json!(22579200),
            Value::Null,json!(0),json!(-352800),json!(352800.5),json!(true),json!("352800x")] {
            let mut j = fixture();let media=&mut j["MediaContainer"]["Metadata"][0]["Media"][1];
            media["audioCodec"]=json!("dsd_lsbf_planar");
            media["Part"][0]["Stream"][0]["codec"]=json!("dsd_lsbf_planar");
            media["Part"][0]["Stream"][0]["samplingRate"]=raw;
            assert!(parse(&j,"42","/chosen").unwrap().dsd_level.is_none());
        }
        // The same number on a PCM stream is never evidence of DSD.
        let mut j=fixture();j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0]["Stream"][0]["samplingRate"]=json!(352800);
        let info=parse(&j,"42","/chosen").unwrap();assert_eq!(info.codec,"FLAC");
        assert_eq!(info.sample_rate,Some(352800));assert!(info.dsd_level.is_none());
    }
    #[test]
    fn source_exact_media_part_and_single_audio_stream_only() {
        let mut j = fixture(); let info = parse(&j,"42","/chosen").unwrap();
        assert_eq!(info.codec,"FLAC"); assert_eq!(info.sample_rate,Some(96000)); assert_eq!(info.bit_depth,Some(24));
        assert_eq!(parse(&j,"42","/other").unwrap().codec,"MP3");
        assert!(parse(&j,"43","/chosen").is_none()); assert!(parse(&j,"42","/missing").is_none());
        let part=j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0].clone();
        j["MediaContainer"]["Metadata"][0]["Media"][0]["Part"].as_array_mut().unwrap().push(part);
        assert!(parse(&j,"42","/chosen").is_none());
        j=fixture(); let s=j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0]["Stream"][0].clone();
        j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0]["Stream"].as_array_mut().unwrap().push(s);
        assert!(parse(&j,"42","/chosen").is_none());
    }
    #[test]
    fn source_missing_invalid_conflicting_fields_never_become_zero_or_proofs() {
        for value in [Value::Null,json!(0),json!(-24),json!(24.5),json!("24x"),json!(" 24"),json!(true),json!(128)] {
            let mut j=fixture();j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0]["Stream"][0]["bitDepth"]=value;
            assert!(parse(&j,"42","/chosen").unwrap().bit_depth.is_none());
        }
        let mut j=fixture(); j["MediaContainer"]["Metadata"][0]["Media"][1]["audioCodec"]=json!("aac");
        assert!(parse(&j,"42","/chosen").is_none());
        j=fixture();j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0].as_object_mut().unwrap().remove("Stream");
        let info=parse(&j,"42","/chosen").unwrap();assert_eq!(info.codec,"FLAC");assert!(info.bit_depth.is_none() && info.sample_rate.is_none());
        for c in ["wavpack","pcm_f32le","aac","dsd_lsbf","private-secret"] {
            let mut j=fixture();j["MediaContainer"]["Metadata"][0]["Media"][1]["audioCodec"]=json!(c);j["MediaContainer"]["Metadata"][0]["Media"][1]["Part"][0]["Stream"][0]["codec"]=json!(c);
            let info=parse(&j,"42","/chosen");
            if c=="private-secret" {assert!(info.is_none());} else {assert!(!serde_json::to_string(&info.unwrap()).unwrap().contains("/chosen"));}
        }
    }
}

#[cfg(test)]
mod http_tests{
 use super::*;
 use tokio::io::{AsyncReadExt,AsyncWriteExt};
 #[tokio::test(flavor="multi_thread",worker_threads=2)]
 async fn facts_use_one_resolved_header_authenticated_read_with_caps_and_no_redirect_or_discovery(){
  for mode in 0..7{
   let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let base=format!("http://{}",listener.local_addr().unwrap());
   let settings=super::super::super::super::Settings{enabled:true,plex_url:base.clone(),plex_token:"FIXTURE_ACCOUNT_TOKEN".into(),plex_server_id:Some("server-one".into()),plex_server_name:None};
   let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let manager=Arc::new(PlexConnectionManager::fixture(&settings,http.clone()));{let mut state=manager.state.lock().unwrap();state.current_base_url=Some(base);state.resolved_token=Some("FIXTURE_RESOLVED_TOKEN".into());}
   let changed=manager.clone();let server=tokio::spawn(async move{
    let(mut stream,_)=listener.accept().await.unwrap();let mut wire=Vec::new();loop{let mut chunk=[0;1024];let n=stream.read(&mut chunk).await.unwrap();assert!(n>0);wire.extend_from_slice(&chunk[..n]);if wire.windows(4).any(|v|v==b"\r\n\r\n"){break;}assert!(wire.len()<8192);}
    let request=String::from_utf8(wire).unwrap();assert!(request.starts_with("GET /library/metadata/42 HTTP/1.1"));assert!(!request.lines().next().unwrap().contains("TOKEN"));assert!(request.to_ascii_lowercase().contains("x-plex-token: fixture_resolved_token"));assert!(!request.contains("FIXTURE_ACCOUNT_TOKEN"));
    let mut json=serde_json::json!({"MediaContainer":{"Metadata":[{"type":"track","ratingKey":"42","Media":[{"audioCodec":"flac","Part":[{"key":"/library/parts/42/file.flac","Stream":[{"streamType":2,"codec":"flac","samplingRate":96000,"bitDepth":24}]}]}]}]}});
    if mode==5{json["MediaContainer"]["machineIdentifier"]=serde_json::json!("other-server");}
    let body=if mode==3||mode==4{vec![b'x';256*1024+1]}else{json.to_string().into_bytes()};
    let status=match mode{1=>"401 Unauthorized",2=>"302 Found",_=>"200 OK"};let extra=if mode==2{"Location: http://127.0.0.1:9/PRIVATE_REDIRECT\r\n"}else{""};
    if mode==6{changed.state.lock().unwrap().generation+=1;}
    let header=if mode==4{format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")}else{format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",body.len())};let _=stream.write_all(header.as_bytes()).await;
    if mode==4{let _=stream.write_all(format!("{:x}\r\n",body.len()).as_bytes()).await;let _=stream.write_all(&body).await;let _=stream.write_all(b"\r\n0\r\n\r\n").await;}else{let _=stream.write_all(&body).await;}drop(stream);
    assert!(tokio::time::timeout(Duration::from_millis(25),listener.accept()).await.is_err(),"optional labels must not retry or follow redirects");
   });
   let client=PlexClient{connection_manager:manager};let locator=MediaLocator::Plex{server_id:"server-one".into(),part_key:"/library/parts/42/file.flac".into(),rating_key:Some("42".into()),file_path:None};let info=client.source_facts(&http,&locator).await;
   if mode==0{let info=info.unwrap();assert_eq!(info.codec,"FLAC");assert_eq!(info.sample_rate,Some(96000));}else{assert!(info.is_none(),"mode {mode}");}server.await.unwrap();
  }
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let settings=super::super::super::super::Settings{enabled:true,plex_url:format!("http://{}",listener.local_addr().unwrap()),plex_token:"FIXTURE".into(),plex_server_id:Some("server-one".into()),plex_server_name:None};let http=reqwest::Client::builder().no_proxy().build().unwrap();let client=PlexClient{connection_manager:Arc::new(PlexConnectionManager::fixture(&settings,http.clone()))};let locator=MediaLocator::Plex{server_id:"server-one".into(),part_key:"/library/parts/42/file.flac".into(),rating_key:Some("42".into()),file_path:None};assert!(client.source_facts(&http,&locator).await.is_none());assert!(tokio::time::timeout(Duration::from_millis(25),listener.accept()).await.is_err());
 }
}

#[cfg(test)]
mod file_info_tests {
    use super::*;
    use serde_json::json;
    fn track() -> Value {
        json!({"type":"track","ratingKey":"42","Media":[{"audioCodec":"flac","audioChannels":2,"bitrate":3100,"container":"flac",
            "Part":[{"key":"/chosen","file":"/private/music.flac","Stream":[{"streamType":2,"codec":"flac","samplingRate":"96000","bitDepth":"24","channels":2,"bitrate":"3000"}]}]}]})
    }
    #[test]
    fn album_track_info_uses_selected_part_and_only_declared_metadata() {
        let info = track_file_info(&track(),"42","/chosen").unwrap();
        assert_eq!(info.codec.as_deref(),Some("FLAC"));assert_eq!(info.container.as_deref(),Some("FLAC"));
        assert_eq!(info.sample_rate,Some(96000));assert_eq!(info.bit_depth,Some(24));assert_eq!(info.channels,Some(2));assert_eq!(info.bitrate_kbps,Some(3000));
        assert!(track_file_info(&track(),"other","/chosen").is_none());assert!(track_file_info(&track(),"42","/other").is_none());
        let mut duplicate=track();let part=duplicate["Media"][0]["Part"][0].clone();duplicate["Media"][0]["Part"].as_array_mut().unwrap().push(part);
        assert!(track_file_info(&duplicate,"42","/chosen").is_none());
    }
    #[test]
    fn dsd_lossy_and_missing_metadata_keep_their_meaning() {
        let mut dsd=track();dsd["Media"][0]["audioCodec"]=json!("dsd_msbf");let part=&mut dsd["Media"][0]["Part"][0];part["container"]=json!("dsf");part["Stream"][0]["codec"]=json!("dsd_msbf");part["Stream"][0]["samplingRate"]=json!(705600);
        let info=track_file_info(&dsd,"42","/chosen").unwrap();assert_eq!(info.sample_rate,Some(5644800));assert_eq!(info.dsd_level,Some(128));assert_eq!(info.sample_type.as_deref(),Some("dsd"));assert_eq!(info.container.as_deref(),Some("DSF"));
        let mut lossy=track();lossy["Media"][0]["audioCodec"]=json!("mp3");lossy["Media"][0]["Part"][0]["Stream"][0]["codec"]=json!("mp3");assert_eq!(track_file_info(&lossy,"42","/chosen").unwrap().sample_type.as_deref(),Some("not_applicable"));
        let empty=json!({"type":"track","ratingKey":"42","Media":[{"Part":[{"key":"/chosen"}]}]});assert!(track_file_info(&empty,"42","/chosen").is_none());
        let mut partial=track();partial["Media"][0]["Part"][0].as_object_mut().unwrap().remove("Stream");let info=track_file_info(&partial,"42","/chosen").unwrap();assert_eq!(info.sample_rate,None);assert_eq!(info.bit_depth,None);assert_eq!(info.channels,Some(2));
    }
    #[test]
    fn untrusted_metadata_never_leaks_paths_tokens_or_invents_ambiguous_channels() {
        let mut j=track();j["Media"][0]["container"]=json!("/private/file?X-Plex-Token=SECRET");j["Media"][0]["audioCodec"]=json!("SECRET");j["Media"][0]["Part"][0]["Stream"][0]["codec"]=json!("SECRET");
        j["Media"][0]["Part"][0]["Stream"][0]["channels"]=json!(1);
        let info=track_file_info(&j,"42","/chosen").unwrap();assert_eq!(info.channels,None);assert_eq!(info.codec,None);let text=serde_json::to_string(&info).unwrap();for private in ["SECRET","private","X-Plex","chosen"] {assert!(!text.contains(private));}
        let audio=j["Media"][0]["Part"][0]["Stream"][0].clone();j["Media"][0]["Part"][0]["Stream"].as_array_mut().unwrap().push(audio);assert!(track_file_info(&j,"42","/chosen").is_none());
    }
    #[test]
    fn legacy_track_dtos_without_audio_info_still_deserialize() {
        let track: super::super::PlexTrack=serde_json::from_value(json!({"rating_key":"42","title":"Synthetic","album_title":null,"artist":null,"thumb":null,"track_index":1,"duration_ms":120000,"media_locator":{"kind":"plex","server_id":"server","part_key":"/chosen","rating_key":"42"}})).unwrap();
        assert!(track.audio_info.is_none());assert!(serde_json::to_value(track).unwrap().get("audio_info").is_none());
    }
}

#[cfg(test)]
mod dto_tests{
 use super::*;
 #[test]
 fn real_album_track_projection_reuses_parser_without_a_request_and_keeps_locator_separate(){
  let settings=super::super::super::super::Settings{enabled:true,plex_url:"http://fixture.invalid".into(),plex_token:"FIXTURE_TOKEN".into(),plex_server_id:Some("server-one".into()),plex_server_name:None};let http=reqwest::Client::builder().no_proxy().build().unwrap();let client=PlexClient{connection_manager:Arc::new(PlexConnectionManager::fixture(&settings,http))};
  let connection=ResolvedPlexConnection{server_id:"server-one".into(),base_url:"http://fixture.invalid".into(),token:"FIXTURE_TOKEN".into(),generation:0};
  let item=serde_json::json!({"type":"track","ratingKey":"42","title":"Fixture","Media":[{"audioCodec":"flac","container":"flac","Part":[{"key":"/library/parts/42/file.flac","file":"PRIVATE_SERVER_PATH","Stream":[{"streamType":2,"codec":"flac","samplingRate":96000,"bitDepth":24,"channels":2}]}]}]});
  let row=client.parse_track_item(&connection,&item).unwrap();assert_eq!(row.audio_info.as_ref().unwrap().sample_rate,Some(96000));let value=serde_json::to_value(row).unwrap();assert_eq!(value["audio_info"]["bit_depth"],24);assert_eq!(value["media_locator"]["rating_key"],"42");assert!(!value.to_string().contains("PRIVATE"));assert!(!value.to_string().contains("FIXTURE_TOKEN"));
 }
}
