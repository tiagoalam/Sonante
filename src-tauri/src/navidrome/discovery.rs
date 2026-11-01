//! Bounded server queries. Discovery never reads the full catalog or changes playback.
use super::{api, current, Album, INVALID, PROTOCOL};
use serde::Serialize;
use serde_json::Value;
const LIMIT: usize = 20;
#[derive(Clone, Serialize)]
pub(crate) struct Artist {
    id: String,
    name: String,
    album_count: Option<u32>,
}
#[derive(Clone, Serialize)]
pub(crate) struct SearchSong {
    #[serde(flatten)]
    song: api::Song,
    album_id: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct SearchResults {
    connection_id: String,
    account_id: String,
    artists: Vec<Artist>,
    albums: Vec<Album>,
    songs: Vec<SearchSong>,
}
fn rows<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a [Value], String> {
    match value.get(key) {
        None => Ok(&[]),
        Some(Value::Array(v)) if v.len() <= max => Ok(v),
        _ => Err(PROTOCOL.into()),
    }
}
fn artist(value: &Value) -> Result<Artist, String> {
    let id = value["id"]
        .as_str()
        .filter(|v| api::id(v))
        .ok_or(PROTOCOL)?
        .to_owned();
    let name = value["name"]
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= 2048 && !v.chars().any(char::is_control))
        .ok_or(PROTOCOL)?
        .to_owned();
    Ok(Artist {
        id,
        name,
        album_count: value["albumCount"]
            .as_u64()
            .filter(|n| *n <= 100_000)
            .map(|n| n as u32),
    })
}
fn search_results(
    connection_id: &str,
    account_id: String,
    root: &Value,
) -> Result<SearchResults, String> {
    let value = root
        .get("searchResult3")
        .filter(|v| v.is_object())
        .ok_or(PROTOCOL)?;
    let artists = rows(value, "artist", LIMIT + 1)?
        .iter()
        .map(artist)
        .collect::<Result<_, _>>()?;
    let albums = rows(value, "album", LIMIT + 1)?
        .iter()
        .map(api::album)
        .collect::<Result<_, _>>()?;
    let songs = rows(value, "song", LIMIT + 1)?
        .iter()
        .map(|v| {
            let song = api::songs(
                &serde_json::json!({"name":v["album"],"artist":v["artist"],"song":[v]}),
            )?
            .into_iter()
            .next()
            .ok_or(PROTOCOL)?;
            let album_id = match v.get("albumId") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) if api::id(id) => Some(id.clone()),
                _ => return Err(PROTOCOL.into()),
            };
            Ok(SearchSong { song, album_id })
        })
        .collect::<Result<_, String>>()?;
    Ok(SearchResults {
        connection_id: connection_id.to_owned(),
        account_id,
        artists,
        albums,
        songs,
    })
}
pub(super) async fn search(connection_id: String, query: String) -> Result<SearchResults, String> {
    let query = query.trim();
    if !(2..=160).contains(&query.chars().count()) || query.chars().any(char::is_control) {
        return Err(INVALID.into());
    }
    let c = current(&connection_id)?;
    let root = c
        .request(
            "search3",
            &[
                ("query", query.into()),
                ("artistCount", (LIMIT + 1).to_string()),
                ("albumCount", (LIMIT + 1).to_string()),
                ("songCount", (LIMIT + 1).to_string()),
            ],
        )
        .await?;
    current(&connection_id)?;
    search_results(
        &connection_id,
        super::favorites::scope(&c.credentials),
        &root,
    )
}
#[derive(Serialize)]
pub(crate) struct ArtistDetail {
    artist: Artist,
    albums: Vec<Album>,
}
pub(super) async fn artist_detail(
    connection_id: String,
    id: String,
) -> Result<ArtistDetail, String> {
    if !api::id(&id) {
        return Err(INVALID.into());
    }
    let c = current(&connection_id)?;
    let root = c.request("getArtist", &[("id", id.clone())]).await?;
    let value = root
        .get("artist")
        .filter(|v| v["id"].as_str() == Some(&id))
        .ok_or(PROTOCOL)?;
    let artist = artist(value)?;
    let albums = rows(value, "album", 5000)?
        .iter()
        .map(api::album)
        .collect::<Result<_, _>>()?;
    current(&connection_id)?;
    Ok(ArtistDetail { artist, albums })
}
#[derive(Serialize)]
pub(crate) struct Explore {
    suggested: Vec<Album>,
    added: Vec<Album>,
}
pub(super) async fn explore(connection_id: String) -> Result<Explore, String> {
    let c = current(&connection_id)?;
    let random_params = [("type", "random".into()), ("size", "18".into())];
    let newest_params = [("type", "newest".into()), ("size", "18".into())];
    let (random, newest) = tokio::try_join!(
        c.request("getAlbumList2", &random_params),
        c.request("getAlbumList2", &newest_params)
    )?;
    let suggested = super::album_rows(&random, "albumList2")?;
    let added = super::album_rows(&newest, "albumList2")?;
    if suggested.len() > 18 || added.len() > 18 {
        return Err(PROTOCOL.into());
    }
    current(&connection_id)?;
    Ok(Explore { suggested, added })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_payload_preserves_album_identity_and_contains_no_authenticated_urls() {
        let root = serde_json::json!({"searchResult3":{"artist":[{"id":"artist","name":"Artist","albumCount":1}],"album":[{"id":"album","name":"Album","artist":"Artist","songCount":1}],"song":[{"id":"song","title":"Track","artist":"Artist","album":"Album","albumId":"album","duration":120,"streamUrl":"https://private.invalid/?password=secret"}]}});
        let parsed = search_results("fixture-session", "a".repeat(64), &root).unwrap();
        assert_eq!(parsed.songs[0].song.album, "Album");
        assert_eq!(parsed.songs[0].album_id.as_deref(), Some("album"));
        assert_eq!(parsed.songs[0].song.duration, Some(120.));
        let safe = serde_json::to_string(&parsed).unwrap();
        for secret in ["password", "streamUrl", "https://private.invalid"] {
            assert!(!safe.contains(secret));
        }
        let mut invalid = root.clone();
        invalid["searchResult3"]["song"][0]["albumId"] = serde_json::json!("../wrong");
        assert!(search_results("fixture-session", "a".repeat(64), &invalid).is_err());
        invalid = root.clone();
        invalid["searchResult3"]["artist"] =
            serde_json::json!(vec![serde_json::json!({"id":"artist","name":"Artist"}); 22]);
        assert!(search_results("fixture-session", "a".repeat(64), &invalid).is_err());
        assert!(search_results(
            "fixture-session",
            "a".repeat(64),
            &serde_json::json!({"searchResult3":[]})
        )
        .is_err());
    }
    #[test]
    fn discovery_parsers_bound_results_and_validate_artist_identity() {
        assert!(artist(&serde_json::json!({"id":"../escape","name":"Artist"})).is_err());
        assert!(artist(&serde_json::json!({"id":"artist","name":""})).is_err());
        assert_eq!(
            artist(&serde_json::json!({"id":"artist","name":"Artist","albumCount":4}))
                .unwrap()
                .album_count,
            Some(4)
        );
        assert!(rows(&serde_json::json!({"album":[{},{}]}), "album", 1).is_err());
        assert!(rows(&serde_json::json!({"album":{}}), "album", 18).is_err());
        assert!(rows(&serde_json::json!({}), "album", 18)
            .unwrap()
            .is_empty());
    }
}
