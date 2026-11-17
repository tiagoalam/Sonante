//! Original Navidrome catalog/detail/artwork policy, with explicit registry leases.
use super::{api::{self, Album, Song}, connection::{Connection,Registry}, INVALID,PROTOCOL,UNAVAILABLE};
use serde::{Deserialize,Serialize};
use serde_json::Value;
use super::CHANGED;
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sort {
    Name,
    Recent,
}
#[derive(Serialize)]
pub(crate) struct Page {
    albums: Vec<Album>,
    offset: usize,
    more: bool,
    total: Option<usize>,
}
pub(super) fn album_rows(root: &Value, key: &str) -> Result<Vec<Album>, String> {
    let parent = root.get(key).filter(|v| v.is_object()).ok_or(PROTOCOL)?;
    match parent.get("album") {
        None => Ok(vec![]),
        Some(Value::Array(values)) if values.len() <= api::PAGE_SIZE => {
            values.iter().map(api::album).collect()
        }
        _ => Err(PROTOCOL.into()),
    }
}
#[derive(Clone, Serialize)]
pub(crate) struct Detail {
    pub(super) album: Album,
    pub(super) songs: Vec<Song>,
    artist_id: Option<String>,
}
impl Detail {
    pub(super) fn size(&self) -> usize {
        self.album.id.len()
            + self.album.title.len()
            + self.album.artist.len()
            + self.album.cover_id.as_ref().map_or(0, String::len)
            + self.artist_id.as_ref().map_or(0, String::len)
            + self
                .songs
                .iter()
                .map(|song| {
                    song.id.len() + song.title.len() + song.artist.len() + song.album.len() + 128
                })
                .sum::<usize>()
    }
}
async fn detail(c: &Connection, id: &str) -> Result<Detail, String> {
    if !api::id(id) {
        return Err(INVALID.into());
    }
    let root = c.request("getAlbum", &[("id", id.into())]).await?;
    let value = root.get("album").ok_or(PROTOCOL)?;
    let album = api::album(value)?;
    if album.id != id {
        return Err(PROTOCOL.into());
    }
    Ok(Detail {
        album,
        songs: api::songs(value)?,
        artist_id: value["artistId"].as_str().filter(|id| api::id(id)).map(str::to_owned),
    })
}
#[derive(Serialize)]
pub(crate) struct PlayerNavigation {
    connection_id: String, song_id: String, account_id: String,
    album_id: String, artist_id: Option<String>,
}
async fn player_navigation(c:&Connection,song_id:&str)->Result<PlayerNavigation,String> {
    if !c.active() {return Err(CHANGED.into());}
    if !api::id(song_id) {return Err(INVALID.into());}
    let root=c.request("getSong",&[("id",song_id.to_owned())]).await?;
    let value=root.get("song").filter(|v|v.is_object()&&v["id"].as_str()==Some(song_id)).ok_or(PROTOCOL)?;
    let album_id=value["albumId"].as_str().filter(|id|api::id(id)).ok_or(PROTOCOL)?.to_owned();
    let artist_id=value["artistId"].as_str().filter(|id|api::id(id)).map(str::to_owned);
    if !c.active() {return Err(CHANGED.into());}
    Ok(PlayerNavigation{connection_id:c.credentials.connection_id.clone(),song_id:song_id.into(),
        account_id:c.account(),album_id,artist_id})
}
async fn song_cover_id_inner(c: &Connection, song: &str) -> Result<Option<String>, String> {
    if !api::id(song) { return Err(INVALID.into()); }
    if !c.active() { return Err(CHANGED.into()); }
    let cached = {
        let details = c.details.lock().map_err(|_| UNAVAILABLE)?;
        details.iter().rev().find(|album| album.songs.iter().any(|item| item.id == song))
            .and_then(|album| album.album.cover_id.clone())
    };
    if !c.active() { return Err(CHANGED.into()); }
    if cached.is_some() { return Ok(cached); }
    // The prepared album may have been evicted while another album was browsed.
    // Resolve the exact song from this connection instead of guessing a cover ID.
    let _slot = c.covers.acquire().await.map_err(|_| UNAVAILABLE)?;
    let root = c.request("getSong", &[("id", song.into())]).await?;
    let item = root.get("song").filter(|value| value.is_object()).ok_or(PROTOCOL)?;
    if item["id"].as_str() != Some(song) { return Err(PROTOCOL.into()); }
    match item.get("coverArt") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if api::id(id) => Ok(Some(id.clone())),
        _ => Err(PROTOCOL.into()),
    }
}

impl Registry {
pub(crate) async fn albums(
    &self,
    connection_id: String,
    query: String,
    sort: Sort,
    offset: usize,
    page_size: Option<usize>,
) -> Result<Page, String> {
    let size = page_size.unwrap_or(36);
    if !matches!(size, 18 | 36 | 72) { return Err(INVALID.into()); }
    if offset > 1_000_000 || query.len() > 256 || query.chars().any(char::is_control) {
        return Err(INVALID.into());
    }
    let c = self.current(&connection_id)?;
    let (root, key, total) = if query.trim().is_empty() {
        if !c.active() { return Err(CHANGED.into()); }
        let (root, total) = c.request_counted( "getAlbumList2", &[
            ("type", match sort { Sort::Name => "alphabeticalByName", Sort::Recent => "newest" }.into()),
            ("size", size.to_string()), ("offset", offset.to_string()),
        ]).await?;
        if !c.active() { return Err(CHANGED.into()); }
        (root, "albumList2", total)
    } else {
        (
            c.request(
                "search3",
                &[("query", query.trim().into()),("artistCount", "0".into()),("songCount", "0".into()),("albumCount", size.to_string()),("albumOffset", offset.to_string())],
            ).await?, "searchResult3", None,
        )
    };
    let albums = album_rows(&root, key)?;
    if albums.len() > size { return Err(PROTOCOL.into()); }
    self.with_current(&c, || Ok(()))?;
    Ok(Page {
        more: total.map_or(albums.len() == size, |total| offset + albums.len() < total),
        total,
        albums,
        offset,
    })
}
pub(crate) async fn album(&self, connection_id: String, id: String) -> Result<Detail, String> {
    let c = self.current(&connection_id)?;
    let value = detail(&c, &id).await?;
    self.with_current(&c, || Ok(()))?;
    c.remember(&value)?;
    Ok(value)
}
    pub(crate) fn prepared(&self, connection_id:&str, album_id:&str) -> Result<Detail,String> {
        self.current(connection_id)?.prepared(album_id)
    }
    pub(crate) async fn search(&self, connection_id:String, query:String) -> Result<super::discovery::SearchResults,String> {
        super::discovery::search(self,connection_id,query).await
    }
    pub(crate) async fn artist_detail(&self, connection_id:String, id:String) -> Result<super::discovery::ArtistDetail,String> {
        super::discovery::artist_detail(self,connection_id,id).await
    }
    pub(crate) async fn explore(&self, connection_id:String) -> Result<super::discovery::Explore,String> {
        super::discovery::explore(self,connection_id).await
    }
    pub(crate) async fn player_navigation(&self,connection_id:String,song_id:String) -> Result<PlayerNavigation,String> {
        let c=self.current(&connection_id)?;
        let result=player_navigation(&c,&song_id).await?;
        self.with_current(&c, || Ok(()))?;
        Ok(result)
    }
    pub(crate) async fn song_cover_id(&self,connection_id:String,song_id:String) -> Result<Option<String>,String> {
        let c=self.current(&connection_id)?;
        let result=c.while_active(song_cover_id_inner(&c,&song_id)).await?;
        self.with_current(&c, || Ok(()))?;
        Ok(result)
    }
    pub(crate) async fn cover(&self,connection_id:String,id:String) -> Result<String,String> {
        use base64::Engine;
        if !api::id(&id) {return Err(INVALID.into());}
        let c=self.current(&connection_id)?;
        let result=c.while_active(async {
            let _slot=c.covers.acquire().await.map_err(|_|UNAVAILABLE)?;
            let response=c.client.get(c.credentials.url("getCoverArt",&[("id",id),("size","256".into())])?)
                .timeout(std::time::Duration::from_secs(12)).send().await.map_err(|_|UNAVAILABLE)?;
            let bytes=api::bytes(response,512*1024).await?;
            let mime=api::cover_type(&bytes).ok_or(PROTOCOL)?;
            Ok(format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes)))
        }).await?;
        self.with_current(&c, || Ok(()))?;
        Ok(result)
    }

}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn album_page_sizes_are_validated_before_any_connection_or_server_access() {
        for size in [None,Some(18),Some(36),Some(72)] {
            assert_eq!(Registry::default().albums("missing-page-fixture".into(),"".into(),Sort::Name,0,size).await.err().as_deref(),Some(CHANGED));
        }
        for size in [0,1,19,60,73,500,usize::MAX] {
            assert_eq!(Registry::default().albums("missing-page-fixture".into(),"".into(),Sort::Name,0,Some(size)).await.err().as_deref(),Some(INVALID));
        }
        let album = serde_json::json!({"id":"album","name":"Album"});
        assert!(album_rows(&serde_json::json!({"albumList2":{"album":vec![album;73]}}),"albumList2").is_err());
    }
    #[test]
    fn malformed_catalog_is_not_a_successful_empty_library() {
        assert!(album_rows(&serde_json::json!({}), "albumList2").is_err());
        assert!(album_rows(
            &serde_json::json!({"albumList2":{"album":null}}),
            "albumList2"
        )
        .is_err());
        assert!(
            album_rows(&serde_json::json!({"albumList2":{}}), "albumList2")
                .unwrap()
                .is_empty()
        );
        assert!(album_rows(
            &serde_json::json!({"albumList2":{"album":[{"name":"No ID"}]}}),
            "albumList2"
        )
        .is_err());
    }
}

#[cfg(test)]
#[path="catalog_http_tests.rs"]
mod windows_contract_tests;
