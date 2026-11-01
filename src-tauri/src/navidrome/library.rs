//! Durable account/song references. No credentials, proxy URLs or session IDs enter stores.
use super::{api, favorites, state, Connection, CHANGED, INVALID, PROTOCOL, UNAVAILABLE};
use crate::audio::{MediaLocator, TrackMetadata};
use std::{collections::HashSet, sync::Arc, time::{Duration, Instant}};
use serde::Serialize;

pub(crate) fn valid_account(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn connection_for_account(id: &str) -> Result<Arc<Connection>, String> {
    if !valid_account(id) || !crate::music_sources::navidrome() { return Err(CHANGED.into()); }
    state().current.lock().map_err(|_| UNAVAILABLE)?.as_ref()
        .filter(|c| c.active() && favorites::scope(&c.credentials) == id).cloned().ok_or(CHANGED.into())
}
pub(crate) fn account(id: &str) -> Result<(), String> { connection_for_account(id).map(|_| ()) }
pub(crate) fn storage_locator(locator: &MediaLocator) -> Option<MediaLocator> {
    let MediaLocator::Navidrome { account_id: Some(account_id), song_id, .. } = locator else { return None; };
    (valid_account(account_id) && api::id(song_id)).then(|| MediaLocator::Navidrome {
        account_id: Some(account_id.clone()), song_id: song_id.clone(), connection_id: String::new(),
    })
}
fn bound(c: &Connection, locator: &MediaLocator) -> Result<MediaLocator, String> {
    let MediaLocator::Navidrome { connection_id, account_id, song_id } = locator else { return Err(INVALID.into()); };
    if !c.active() || !api::id(song_id) || match account_id {
        Some(account) => !valid_account(account) || *account != favorites::scope(&c.credentials),
        None => *connection_id != c.credentials.connection_id,
    } { return Err(CHANGED.into()); }
    Ok(MediaLocator::Navidrome { connection_id: c.credentials.connection_id.clone(), account_id: Some(favorites::scope(&c.credentials)), song_id: song_id.clone() })
}
pub(crate) fn bind_tracks(tracks: &mut [TrackMetadata]) -> Result<(), String> {
    if !tracks.iter().any(|t| matches!(t.media_locator, Some(MediaLocator::Navidrome { .. }))) { return Ok(()); }
    let c = state().current.lock().map_err(|_| UNAVAILABLE)?.as_ref().filter(|c| c.active()).cloned().ok_or(CHANGED)?;
    let values = tracks.iter().map(|t| match &t.media_locator { Some(l @ MediaLocator::Navidrome { .. }) => bound(&c,l).map(Some), other => Ok(other.clone()) }).collect::<Result<Vec<_>,String>>()?;
    for (track, value) in tracks.iter_mut().zip(values) { track.media_locator = value; }
    Ok(())
}
pub(crate) fn with_bound_tracks<T>(tracks: Vec<TrackMetadata>, work: impl FnOnce(Vec<TrackMetadata>) -> Result<T,String>) -> Result<T,String> {
    if !tracks.iter().any(|t|matches!(t.media_locator,Some(MediaLocator::Navidrome{..}))) {return work(tracks);}
    // Called after taking Audio, matching the album player's existing lock order.
    // Hold this lease through the actual queue effect; reconnect cannot swap the proxy meanwhile.
    let guard=state().current.lock().map_err(|_|UNAVAILABLE)?;
    let c=guard.as_ref().filter(|c|c.active()).ok_or(CHANGED)?;
    for track in &tracks {
        if let Some(locator @ MediaLocator::Navidrome{connection_id,..})=&track.media_locator {
            if *connection_id!=c.credentials.connection_id {return Err(CHANGED.into());}
            bound(c,locator)?;
        }
    }
    let result=work(tracks); drop(guard); result
}

async fn song(locator: &MediaLocator) -> Result<(Arc<Connection>, serde_json::Value), String> {
    let MediaLocator::Navidrome { account_id: Some(account_id), song_id, .. } = locator else { return Err(INVALID.into()); };
    let c = connection_for_account(account_id)?;
    if !api::id(song_id) { return Err(INVALID.into()); }
    let root = c.request("getSong", &[("id",song_id.clone())]).await?;
    let value = root.get("song").filter(|v| v["id"].as_str() == Some(song_id.as_str())).ok_or(PROTOCOL)?.clone();
    Ok((c,value))
}
pub(crate) async fn resolve_saved(locator: &MediaLocator) -> Result<(String, MediaLocator), String> {
    let (c, _) = song(locator).await?;
    let MediaLocator::Navidrome {song_id,..}=locator else {return Err(INVALID.into())};
    Ok((c.stream(song_id)?, bound(&c,locator)?))
}
pub(crate) async fn available(locator: &MediaLocator) -> Result<bool, String> { song(locator).await.map(|_| true) }
#[derive(Serialize)]
pub(crate) struct SongNavigation { account_id: String, album_id: String }
pub(super) async fn song_navigation(app: tauri::AppHandle, locator: MediaLocator) -> Result<SongNavigation, String> {
    if !crate::music_sources::navidrome() { return Err(CHANGED.into()); }
    super::navidrome_status(app).await?;
    let (c,value) = song(&locator).await?;
    let album_id = value["albumId"].as_str().filter(|v| api::id(v)).ok_or(PROTOCOL)?.to_string();
    if !c.active() { return Err(CHANGED.into()); }
    Ok(SongNavigation { account_id:favorites::scope(&c.credentials),album_id })
}
fn metadata(value: &serde_json::Value) -> Result<crate::smart_playlists::Album, String> {
    let a = api::album(value)?;
    let mut genres = Vec::new();
    if let Some(genre) = value["genre"].as_str() { genres.push(genre.to_string()); }
    if let Some(values) = value["genres"].as_array() { for v in values.iter().take(64) {
        if let Some(genre) = v["name"].as_str().or_else(|| v.as_str()) { genres.push(genre.to_string()); }
    } }
    if genres.iter().any(|v| v.len()>256 || v.chars().any(char::is_control)) { return Err(PROTOCOL.into()); }
    genres.sort(); genres.dedup();
    let created = value["created"].as_str().filter(|v| v.ends_with('Z') || v.ends_with("+00:00"))
        .and_then(|v| v.get(..19)).map(|v| format!("{v}Z"));
    Ok(crate::smart_playlists::Album { id:a.id.clone(),title:a.title,artist:a.artist,genres,year:a.year,
        added_at:crate::smart_playlists::local_added(created.as_deref()),favorite_key:a.id,image:None })
}
pub(crate) async fn smart_catalog(id: &str) -> Result<Vec<crate::smart_playlists::Album>, String> {
    let c = connection_for_account(id)?;
    // Cache belongs to one live account. It is only populated by explicit smart-playlist work.
    let mut cache = c.catalog.lock().await;
    if let Some((at, albums)) = cache.as_ref().filter(|(at,_)| at.elapsed()<Duration::from_secs(30)) {
        let _ = at;
        if !c.active() { return Err(CHANGED.into()); }
        return Ok(albums.clone());
    }
    let mut albums=Vec::new(); let mut ids=HashSet::new(); let mut offset=0; let mut expected=None;
    loop {
        if !c.active() { return Err(CHANGED.into()); }
        let (root,total)=api::request_counted(&c.client,&c.credentials,"getAlbumList2",&[("type","alphabeticalByName".into()),("size","72".into()),("offset",offset.to_string())]).await?;
        if !c.active() { return Err(CHANGED.into()); }
        if expected.is_some() && total.is_some() && total != expected { return Err(PROTOCOL.into()); }
        if expected.is_none() { expected=total; }
        let rows=root.get("albumList2").filter(|v|v.is_object()).ok_or(PROTOCOL)?.get("album");
        let rows=match rows {None=>&[][..],Some(serde_json::Value::Array(rows)) if rows.len()<=72=>rows.as_slice(),_=>return Err(PROTOCOL.into())};
        let before=albums.len();
        for row in rows { let a=metadata(row)?; if ids.insert(a.id.clone()) { albums.push(a); } }
        offset+=rows.len();
        if albums.len()>100_000 { return Err(PROTOCOL.into()); }
        if rows.len()<72 || expected.is_some_and(|n|offset>=n) { break; }
        if albums.len()==before { return Err(PROTOCOL.into()); }
    }
    if expected.is_some_and(|n|n!=albums.len()) { return Err(PROTOCOL.into()); }
    if !c.active() { return Err(CHANGED.into()); }
    *cache=Some((Instant::now(),albums.clone())); Ok(albums)
}
pub(crate) async fn smart_favorites(id: &str) -> Result<Vec<String>, String> {
    let c=connection_for_account(id)?; let own=id.to_owned();
    let result=tauri::async_runtime::spawn_blocking(move||favorites::read(&super::path().with_file_name("navidrome-favorites.json"),&own)).await.map_err(|_|UNAVAILABLE)??;
    if !c.active() { return Err(CHANGED.into()); }
    Ok(result.into_iter().map(|a|a.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(username: &str, session: &str) -> Connection {
        Connection {credentials:api::Credentials {connection_id:session.into(),server:"http://fixture.invalid/".into(),username:username.into(),password:"PRIVATE_PASSWORD".into()},client:api::client().unwrap(),port:1234,secret:"PRIVATE_PROXY".into(),live:std::sync::atomic::AtomicBool::new(true),stop:Arc::new(tokio::sync::Notify::new()),covers:tokio::sync::Semaphore::new(3),details:std::sync::Mutex::new(std::collections::VecDeque::new()),catalog:tokio::sync::Mutex::new(None)}
    }
    #[test]
    fn saved_reference_survives_reconnect_but_never_crosses_accounts_or_contains_sessions() {
        let before=fixture("one","OLD_SESSION");let after=fixture("one","NEW_SESSION");let other=fixture("two","NEW_SESSION");
        let live=MediaLocator::Navidrome {connection_id:"OLD_SESSION".into(),song_id:"song-a".into(),account_id:Some(favorites::scope(&before.credentials))};
        let saved=storage_locator(&live).unwrap();let bytes=serde_json::to_string(&saved).unwrap();
        for secret in ["OLD_SESSION","NEW_SESSION","PRIVATE_PASSWORD","PRIVATE_PROXY","fixture.invalid"] {assert!(!bytes.contains(secret));}
        let rebound=bound(&after,&saved).unwrap();assert!(matches!(rebound,MediaLocator::Navidrome{connection_id,..} if connection_id=="NEW_SESSION"));
        assert!(bound(&other,&saved).is_err());
        assert!(storage_locator(&MediaLocator::Navidrome{connection_id:"old".into(),song_id:"song".into(),account_id:None}).is_none());
        for song in ["../escape","http://private/?token=SECRET",""] {assert!(storage_locator(&MediaLocator::Navidrome{connection_id:String::new(),song_id:song.into(),account_id:Some(favorites::scope(&after.credentials))}).is_none());}
    }
    #[test]
    fn smart_metadata_uses_server_genres_and_added_date_without_guessing_unknown_fields() {
        let a=metadata(&serde_json::json!({"id":"album","name":"Album","artist":"Artist","genre":"Jazz","genres":[{"name":"Jazz"},{"name":"Fusion"}],"created":"2026-11-01T08:30:00.123Z","year":1974})).unwrap();
        assert_eq!(a.genres,vec!["Fusion","Jazz"]);assert!(a.added_at.is_some());assert_eq!(a.year,Some(1974));
        let a=metadata(&serde_json::json!({"id":"album","name":"Album"})).unwrap();assert!(a.genres.is_empty());assert!(a.added_at.is_none());
    }
    #[test]
    fn queue_effect_holds_the_live_session_and_refuses_retired_references_before_effects() {
        let c=Arc::new(fixture("one","LIVE_SESSION"));let account=favorites::scope(&c.credentials);
        let previous={let mut guard=state().current.lock().unwrap();guard.replace(c)};
        let track=TrackMetadata{title:"Fixture".into(),artist:String::new(),album:String::new(),uri:String::new(),thumb:None,plex_image:None,duration:None,media_locator:Some(MediaLocator::Navidrome{connection_id:"LIVE_SESSION".into(),account_id:Some(account),song_id:"song-a".into()})};
        with_bound_tracks(vec![track.clone()],|tracks|{assert_eq!(tracks.len(),1);assert!(state().current.try_lock().is_err(),"session remains leased through the queue effect");Ok(())}).unwrap();
        let mut old=track;if let Some(MediaLocator::Navidrome{connection_id,..})=&mut old.media_locator{*connection_id="RETIRED".into();}
        assert!(with_bound_tracks::<()>(vec![old],|_|panic!("retired reference reached a queue effect")).is_err());
        *state().current.lock().unwrap()=previous;
    }

    #[test]
    fn restored_queue_rebinds_only_the_saved_account_and_revalidates_the_session_before_effects() {
        let first = Arc::new(fixture("one", "NEW_SESSION"));
        let id = favorites::scope(&first.credentials);
        let previous = state().current.lock().unwrap().replace(first.clone());
        let make = |song: &str| TrackMetadata {title:"Fixture".into(),artist:"Artist".into(),album:"Album".into(),uri:String::new(),thumb:None,plex_image:None,duration:Some(120.),media_locator:Some(MediaLocator::Navidrome{connection_id:String::new(),account_id:Some(id.clone()),song_id:song.into()})};
        let saved = vec![make("one"), make("two"), make("one")];
        let mut rebound = saved.clone();
        bind_tracks(&mut rebound).unwrap();
        assert_eq!(rebound[0].media_locator, rebound[2].media_locator);
        for track in &rebound {
            assert!(matches!(&track.media_locator, Some(MediaLocator::Navidrome {connection_id,..}) if connection_id == "NEW_SESSION"));
        }
        let second = Arc::new(fixture("one", "RECONNECTED"));
        *state().current.lock().unwrap() = Some(second.clone()); first.retire();
        assert!(with_bound_tracks::<()>(rebound, |_| panic!("old proxy reached restore effect")).is_err());
        let mut fresh = saved.clone(); bind_tracks(&mut fresh).unwrap();
        with_bound_tracks(fresh, |tracks| {
            assert_eq!(tracks.len(), 3);
            assert!(state().current.try_lock().is_err(), "live proxy is leased through the restore effect");
            Ok(())
        }).unwrap();
        *state().current.lock().unwrap() = Some(Arc::new(fixture("other-account", "RECONNECTED"))); second.retire();
        let mut wrong = saved.clone(); assert!(bind_tracks(&mut wrong).is_err());
        assert_eq!(wrong[0].media_locator, saved[0].media_locator, "failed binding must not partly mutate the captured queue");
        *state().current.lock().unwrap() = previous;
    }

}
