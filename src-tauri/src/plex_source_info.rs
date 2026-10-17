//! PMS source metadata only. Exact ratingKey + Part key; never stream or extension inference.
//! Contracts: developer.plex.tv/pms and python-plexapi.readthedocs.io/en/latest/modules/media.html
//! AudioStream: streamType=2, codec, bitDepth, samplingRate; Media.audioCodec; Part.key.
use super::*;
use crate::player_source_info::SourceAudioFacts;
use serde_json::Value;
const MAX_BODY: usize = 256 * 1024;
const DEADLINE: Duration = Duration::from_secs(4);

pub(super) fn source_library(item: &Value) -> Option<String> {
    let value = item.get("librarySectionID")?;
    let id = value.as_str().map(str::to_owned).or_else(|| value.as_u64().map(|n| n.to_string()))?;
    navigation_rating_key(&id).then_some(id)
}
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
fn parse(json: &Value, rating: &str, part_key: &str) -> Option<SourceAudioFacts> {
    let metadata = json.get("MediaContainer")?.get("Metadata")?.as_array()?;
    if metadata.len() > 4 { return None; }
    let tracks: Vec<_> = metadata.iter().filter(|t| t["ratingKey"].as_str() == Some(rating)).collect();
    if tracks.len() != 1 || tracks[0]["type"].as_str() != Some("track") { return None; }
    let medias = tracks[0].get("Media")?.as_array()?;
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

fn album_parse(json: &Value, library: &str, album: &str) -> Option<Vec<SourceAudioFacts>> {
    let missing = |reason| crate::player_source_info::album_source_absent("plex_parse", reason, 0);
    let container = json.get("MediaContainer").or_else(|| crate::player_source_info::album_source_absent("plex_parse", "container", 0))?;
    if source_library(container).as_deref() != Some(library) { return missing("library"); }
    let count = uint(&container["totalSize"], 32).or_else(|| crate::player_source_info::album_source_absent("plex_parse", "inventory_total", 0))? as usize;
    if uint(&container["size"],32)? as usize != count { return missing("inventory_size"); }
    if container.get("offset").is_some_and(|o| o.as_u64() != Some(0) && o.as_str() != Some("0")) { return None; }
    let tracks = container.get("Metadata")?.as_array()?;
    if tracks.len() != count { return missing("inventory_count"); }
    let mut seen = HashSet::new(); let mut facts = Vec::with_capacity(count);
    for track in tracks {
        if track["parentRatingKey"].as_str() != Some(album) { return None; }
        if track.get("librarySectionID").is_some_and(|_| source_library(track).as_deref() != Some(library)) { return None; }
        let rating = track["ratingKey"].as_str()?;
        if !navigation_rating_key(rating) || !seen.insert(rating) { return None; }
        let medias = track.get("Media")?.as_array()?;
        if medias.len() != 1 { return None; }
        let parts = medias[0].get("Part")?.as_array()?;
        if parts.len() != 1 { return None; }
        let key = parts[0]["key"].as_str()?;
        if !is_valid_plex_path(key) || contains_plex_token(key) { return None; }
        let single = serde_json::json!({"MediaContainer":{"Metadata":[track]}});
        facts.push(parse(&single, rating, key)?);
    }
    Some(facts)
}

fn http() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(DEADLINE).build().ok()).as_ref()
}
impl PlexClient {
    pub(crate) fn source_revision(&self) -> Option<u64> { Some(self.connection_manager.snapshot().ok()?.generation) }
    pub(crate) fn source_context_matches(&self, original: &Self, revision: u64) -> bool {
        Arc::ptr_eq(&self.connection_manager, &original.connection_manager) && self.source_revision() == Some(revision)
    }
    async fn source_json(&self, server_id: &str, path: &str, children: bool) -> Option<Value> {
        // Read an already authenticated/resolved route from the existing manager.
        // Optional labels must not cause rediscovery, recovery, probes or retries.
        let manager = &self.connection_manager;
        let before = manager.snapshot().ok()?;
        if before.identity.as_ref()?.machine_identifier != server_id { return None; }
        let base = before.current_base_url.as_ref()?;
        let token = before.resolved_token.as_ref().unwrap_or(&before.account_token);
        let mut url = reqwest::Url::parse(base).ok()?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
            || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() { return None; }
        url.set_path(path);
        if children { url.set_query(Some("X-Plex-Container-Start=0&X-Plex-Container-Size=33")); }
        let mut response = http()?.get(url).header("X-Plex-Token", token).header("Accept", "application/json")
            .header("X-Plex-Client-Identifier", PLEX_CLIENT_ID).send().await.ok()?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_BODY as u64) { return None; }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if body.len().saturating_add(chunk.len()) > MAX_BODY { return None; } body.extend_from_slice(&chunk);
        }
        let json: Value = serde_json::from_slice(&body).ok()?;
        let after = manager.snapshot().ok()?;
        if after.generation != before.generation || after.identity.as_ref()?.machine_identifier != server_id { return None; }
        if let Some(id) = json["MediaContainer"].get("machineIdentifier") { if id.as_str() != Some(server_id) { return None; } }
        Some(json)
    }
    pub(crate) async fn album_source_audio_facts(&self, server: &str, library: &str, album: &str) -> Option<Vec<SourceAudioFacts>> {
        if validate_plex_server_id(server).is_err() || !navigation_rating_key(library) || !navigation_rating_key(album) { return None; }
        let json = self.source_json(server, &format!("/library/metadata/{album}/children"), true).await?;
        album_parse(&json, library, album)
    }
    pub(crate) async fn source_audio_facts(&self, locator: &MediaLocator) -> Option<SourceAudioFacts> {
        let MediaLocator::Plex { server_id, part_key, rating_key: Some(rating), .. } = locator else { return None; };
        if validate_plex_server_id(server_id).is_err() || !navigation_rating_key(rating)
            || !is_valid_plex_path(part_key) || part_key.len() > 4096 || part_key.contains(['?', '#']) || contains_plex_token(part_key) { return None; }
        let json = self.source_json(server_id, &format!("/library/metadata/{rating}"), false).await?;
        // Output contains only fixed labels and bounded numbers, no PMS strings/paths/tokens.
        parse(&json, rating, part_key)
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
    #[test]
    fn source_info_album_rejects_incomplete_foreign_duplicates_and_ambiguous_renditions() {
        let mut track=fixture()["MediaContainer"]["Metadata"][0].clone();
        track["parentRatingKey"]=json!("12");
        track["Media"].as_array_mut().unwrap().remove(0);
        let j=json!({"MediaContainer":{"librarySectionID":"7","totalSize":1,"size":1,"offset":0,"Metadata":[track]}});
        assert_eq!(album_parse(&j,"7","12").unwrap().len(),1);
        assert!(album_parse(&j,"8","12").is_none());assert!(album_parse(&j,"7","13").is_none());
        for key in ["totalSize","size","Metadata"] { let mut bad=j.clone();bad["MediaContainer"].as_object_mut().unwrap().remove(key);assert!(album_parse(&bad,"7","12").is_none()); }
        for count in [0,2,33] { let mut bad=j.clone();bad["MediaContainer"]["totalSize"]=json!(count);assert!(album_parse(&bad,"7","12").is_none()); }
        let mut bad=j.clone();bad["MediaContainer"]["offset"]=json!(1);assert!(album_parse(&bad,"7","12").is_none());
        let mut bad=j.clone();let media=bad["MediaContainer"]["Metadata"][0]["Media"][0].clone();bad["MediaContainer"]["Metadata"][0]["Media"].as_array_mut().unwrap().push(media);assert!(album_parse(&bad,"7","12").is_none());
        let mut bad=j.clone();bad["MediaContainer"]["Metadata"][0]["librarySectionID"]=json!("8");assert!(album_parse(&bad,"7","12").is_none());
        let mut bad=j.clone();let track=bad["MediaContainer"]["Metadata"][0].clone();bad["MediaContainer"]["Metadata"].as_array_mut().unwrap().push(track);bad["MediaContainer"]["size"]=json!(2);bad["MediaContainer"]["totalSize"]=json!(2);assert!(album_parse(&bad,"7","12").is_none());
    }
    #[test]
    fn source_info_http_is_single_bounded_read_without_redirect_retry_or_unresolved_probes() {
        use std::io::{Read,Write}; use std::net::TcpListener; use std::thread;
        for (status,extra,body) in [("401 Unauthorized","", "{}".to_string()),
            ("302 Found","Location: http://127.0.0.1:1/private\r\n", "{}".to_string()),
            ("200 OK","", "x".repeat(MAX_BODY+1)), ("200 OK","", fixture().to_string())] {
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();let base=format!("http://{}",listener.local_addr().unwrap());
            let server=thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();let mut request=[0;4096];let n=stream.read(&mut request).unwrap();let req=std::str::from_utf8(&request[..n]).unwrap();
                assert!(req.starts_with("GET /library/metadata/42 HTTP/1.1"));assert!(!req.contains("?X-Plex-Token="));
                let header=format!("HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());let _=stream.write_all(header.as_bytes());let _=stream.write_all(body.as_bytes());});
            let config=AppConfig{plex_server_id:Some("fixture-server".into()),plex_token:"fixture-secret".into(),..AppConfig::default()};
            let manager=PlexConnectionManager::from_config(&config);
            manager.state.lock().unwrap().current_base_url=Some(base);
            let client=PlexClient{connection_manager:Arc::new(manager),playback_mode:"http".into(),path_mappings:HashMap::new()};
            let value=tauri::async_runtime::block_on(client.source_json("fixture-server","/library/metadata/42",false));
            if status=="200 OK" && value.is_some() {assert_eq!(value.unwrap(),fixture());}
            else {assert!(value.is_none());}
            server.join().unwrap();
        }
        let client=super::super::tests::media_client_for_test(&AppConfig::default());
        assert!(tauri::async_runtime::block_on(client.source_json("other","/library/metadata/42",false)).is_none());
    }

    #[test]
    fn source_info_context_rejects_replacement_and_same_server_generation_changes() {
        let config = AppConfig::default();
        let client = super::super::tests::media_client_for_test(&config);
        let clone = client.clone(); let revision = client.source_revision().unwrap();
        assert!(clone.source_context_matches(&client, revision));
        let replacement = super::super::tests::media_client_for_test(&config);
        assert!(!replacement.source_context_matches(&client, revision));
        client.connection_manager.state.lock().unwrap().generation += 1;
        assert!(!clone.source_context_matches(&client, revision));
    }

    #[test]
    fn source_album_accepts_numeric_library_section_contract_without_changing_completeness() {
        // Independent JSON fixture: librarySectionID is an integer in the PMS/PlexAPI contract.
        let j: Value = serde_json::from_str(include_str!("../tests/fixtures/plex_source_album_numeric_section.json")).unwrap();
        let facts = album_parse(&j, "7", "12").expect("a complete numeric-section album is valid");
        assert_eq!(facts.len(), 1); assert_eq!(facts[0].codec, "FLAC");
        assert_eq!(facts[0].sample_rate, Some(96000)); assert_eq!(facts[0].bit_depth, Some(24));
        assert!(album_parse(&j, "8", "12").is_none());
        for bad in [serde_json::Value::Null, json!(7.5), json!(-7), json!(true), json!(" 7"), json!("../7")] {
            let mut invalid = j.clone(); invalid["MediaContainer"]["librarySectionID"] = bad;
            assert!(album_parse(&invalid, "7", "12").is_none());
        }
        let mut incomplete = j.clone(); incomplete["MediaContainer"].as_object_mut().unwrap().remove("totalSize");
        assert!(album_parse(&incomplete, "7", "12").is_none(), "completeness remains mandatory");
        println!("fixture_plex_facts={}", serde_json::to_string(&facts).unwrap());
    }

}
