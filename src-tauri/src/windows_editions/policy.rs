//! Original finite inventory/profile policies; no filesystem or playback.
use std::collections::HashSet;
const MAX_FILES: usize = 500;
const MAX_SAFE_BYTES: u64 = 9_007_199_254_740_991;
#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag="kind",rename_all="snake_case")]
pub(crate) enum Request {
    Local {album_id:String},
    Plex {server_id:String,library_id:String,album_id:String},
    Navidrome {connection_id:String,account_id:String,album_id:String},
}
fn number(value:&serde_json::Value)->Option<u64> {
    let n=value.as_u64().or_else(||{let s=value.as_str()?;if s.is_empty()||s.len()>16||!s.bytes().all(|b|b.is_ascii_digit()){return None;}s.parse().ok()})?;
    (n>0&&n<=MAX_SAFE_BYTES).then_some(n)
}
#[derive(serde::Serialize, Debug)]
pub(crate) struct Profile {
    pub sizes: Option<Vec<u64>>,
    pub titles: Option<Vec<String>>,
    pub cover_id: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub edition_title: Option<String>,
}
impl Profile {
    pub(crate) fn new(sizes: Option<Vec<u64>>, titles: impl IntoIterator<Item=Option<String>>) -> Self {
        let titles:Option<Vec<String>>=titles.into_iter().map(|title|title.filter(|t|!t.trim().is_empty()&&t.len()<=4096&&!t.chars().any(char::is_control))).collect();
        Self {sizes,titles:titles.filter(|t|!t.is_empty()&&t.len()<=MAX_FILES),cover_id:None,edition_title:None}
    }
}
fn plex_rows<'a>(json:&'a serde_json::Value,library:&str,album:&str)->Option<&'a Vec<serde_json::Value>> {
    let c=json.get("MediaContainer")?;
    if c["librarySectionID"].as_str().map(str::to_owned).or_else(||c["librarySectionID"].as_u64().map(|n|n.to_string()))?.as_str()!=library {return None;}
    let rows=c["Metadata"].as_array()?;
    if rows.is_empty()||rows.len()>MAX_FILES||number(&c["size"])?!=rows.len() as u64
        ||number(&c["totalSize"])?!=rows.len() as u64||c.get("offset").is_some_and(|n|n.as_u64()!=Some(0)&&n.as_str()!=Some("0")){return None;}
    let mut ids=HashSet::new();let mut files=HashSet::new();
    for row in rows {
        let id=row["ratingKey"].as_str()?;
        if row["type"].as_str()!=Some("track")||row["parentRatingKey"].as_str()!=Some(album)||id.is_empty()||!ids.insert(id){return None;}
        let media=row["Media"].as_array()?;if media.len()!=1{return None;}let parts=media[0]["Part"].as_array()?;if parts.len()!=1{return None;}
        let file=parts[0]["file"].as_str()?;if file.is_empty()||!files.insert(file){return None;}
    }
    Some(rows)
}
pub(crate) fn plex(json:&serde_json::Value,library:&str,album:&str)->Option<Vec<u64>> {
    plex_rows(json,library,album)?.iter().map(|row|number(&row["Media"][0]["Part"][0]["size"])).collect()
}
pub(crate) fn plex_profile(json:&serde_json::Value,library:&str,album:&str)->Option<Profile> {
    let rows=plex_rows(json,library,album)?;
    let mut profile=Profile::new(plex(json,library,album), rows.iter().map(|r|r["title"].as_str().map(str::to_owned)));
    let folders: Option<Vec<String>> = rows.iter().map(|row| plex_album_folder(row["Media"][0]["Part"][0]["file"].as_str()?)).collect();
    profile.edition_title=folders.and_then(|folders| {
        let first=folders.first()?;
        folders.iter().all(|folder|folder==first).then(||first.clone())
    });
    Some(profile)
}
// Pure projection of Plex metadata; no file access. Disc directories may vary,
// but every track must identify the same album directory. Unknown stays unknown.
fn plex_album_folder(file: &str) -> Option<String> {
    if file.len()>4096 || file.chars().any(char::is_control) || file.contains("://") { return None; }
    let parts:Vec<&str>=file.split(['/', '\\']).filter(|part|!part.is_empty()).collect();
    let mut index=parts.len().checked_sub(2)?;
    let disc=parts[index].to_ascii_lowercase().replace([' ', '-', '_'], "");
    if ["cd", "disc", "disk"].iter().any(|prefix|disc.strip_prefix(prefix).is_some_and(|n|!n.is_empty()&&n.len()<=3&&n.bytes().all(|b|b.is_ascii_digit()))) { index=index.checked_sub(1)?; }
    let folder=parts[index];
    (!folder.is_empty() && folder.len()<=1024 && folder!="." && folder!=".." && !folder.contains(':')).then(||folder.to_owned())
}
fn navidrome_rows<'a>(value:&'a serde_json::Value,album:&str)->Option<&'a Vec<serde_json::Value>> {
    if value["id"].as_str()!=Some(album){return None;}let rows=value["song"].as_array()?;
    if rows.is_empty()||rows.len()>MAX_FILES||number(&value["songCount"])?!=rows.len() as u64{return None;}
    let mut ids=HashSet::new();let mut files=HashSet::new();
    for row in rows {
        let id=row["id"].as_str()?;
        if id.is_empty()||!ids.insert(id)||row.get("albumId").is_some_and(|v|v.as_str()!=Some(album)){return None;}
        if let Some(path)=row["path"].as_str(){if !files.insert(path)||path.to_ascii_lowercase().contains(".cue/"){return None;}}
    }
    Some(rows)
}
pub(crate) fn navidrome(value:&serde_json::Value,album:&str)->Option<Vec<u64>> {
    navidrome_rows(value,album)?.iter().map(|row|number(&row["size"])).collect()
}
pub(crate) fn navidrome_profile(value:&serde_json::Value,album:&str)->Option<Profile> {
    let rows=navidrome_rows(value,album)?;
    let mut profile=Profile::new(navidrome(value,album),rows.iter().map(|r|r["title"].as_str().map(str::to_owned)));
    profile.cover_id=value["coverArt"].as_str().filter(|id|!id.is_empty() && id.len()<=512 && id.bytes().all(|b|b.is_ascii_alphanumeric()||b"._-".contains(&b))).map(str::to_owned);
    Some(profile)
}

#[cfg(test)]mod tests{use super::*;
    #[test] fn missing_zero_negative_float_and_unsafe_numbers_are_never_sizes() {
        for value in [serde_json::json!(null),serde_json::json!(0),serde_json::json!(-1),serde_json::json!(1.5),serde_json::json!(9_007_199_254_740_992u64),serde_json::json!("1e3")] {assert_eq!(number(&value),None);}
        assert_eq!(number(&serde_json::json!("123456789")),Some(123456789));
    }
    #[test] fn plex_requires_complete_matching_album_and_original_file_sizes() {
        let mut value=serde_json::json!({"MediaContainer":{"librarySectionID":7,"size":2,"totalSize":2,"offset":0,"Metadata":[
            {"type":"track","ratingKey":"11","parentRatingKey":"8","Media":[{"Part":[{"file":"/music/a.flac","size":123}]}]},
            {"type":"track","ratingKey":"12","parentRatingKey":"8","Media":[{"Part":[{"file":"/music/b.flac","size":"456"}]}]}]}});
        assert_eq!(plex(&value,"7","8"),Some(vec![123,456]));assert_eq!(plex(&value,"9","8"),None);assert_eq!(plex(&value,"7","9"),None);
        value["MediaContainer"]["totalSize"]=serde_json::json!(3);assert_eq!(plex(&value,"7","8"),None);
        value["MediaContainer"]["totalSize"]=serde_json::json!(2);value["MediaContainer"]["Metadata"][1]["Media"][0]["Part"][0]["size"]=serde_json::Value::Null;
        assert_eq!(plex(&value,"7","8"),None);
    }
    #[test] fn navidrome_requires_all_members_and_rejects_repeated_or_missing_file_sizes() {
        let mut value=serde_json::json!({"id":"album","songCount":2,"song":[{"id":"a","albumId":"album","size":123,"path":"a.flac"},{"id":"b","albumId":"album","size":456,"path":"b.flac"}]});
        assert_eq!(navidrome(&value,"album"),Some(vec![123,456]));assert_eq!(navidrome(&value,"other"),None);
        value["songCount"]=serde_json::json!(3);assert_eq!(navidrome(&value,"album"),None);value["songCount"]=serde_json::json!(2);
        value["song"][1]["path"]=serde_json::json!("a.flac");assert_eq!(navidrome(&value,"album"),None);
        value["song"][1]["path"]=serde_json::json!("b.flac");value["song"][1]["size"]=serde_json::Value::Null;assert_eq!(navidrome(&value,"album"),None);
    }
    #[test] fn profiles_reuse_the_same_complete_inventory_and_keep_titles_without_inventing_missing_sizes() {
        let mut nav=serde_json::json!({"id":"album","songCount":2,"coverArt":"al-123","song":[
            {"id":"a","albumId":"album","title":"One","size":123},
            {"id":"b","albumId":"album","title":"Two"}]});
        let p=navidrome_profile(&nav,"album").unwrap();
        assert_eq!(p.sizes,None);assert_eq!(p.titles,Some(vec!["One".into(),"Two".into()]));assert_eq!(p.cover_id,Some("al-123".into()));
        nav["song"][1]["id"]=serde_json::json!("a");assert!(navidrome_profile(&nav,"album").is_none());
        let mut plex=serde_json::json!({"MediaContainer":{"librarySectionID":7,"size":1,"totalSize":1,"Metadata":[
            {"type":"track","ratingKey":"a","parentRatingKey":"album","title":"One","Media":[{"Part":[{"file":"/music/one.flac"}]}]}]}});
        let p=plex_profile(&plex,"7","album").unwrap();assert_eq!(p.titles,Some(vec!["One".into()]));assert_eq!(p.sizes,None);
        plex["MediaContainer"]["totalSize"]=serde_json::json!(2);assert!(plex_profile(&plex,"7","album").is_none());
    }
    #[test] fn invalid_titles_are_not_tracklist_evidence_and_profiles_never_contain_paths_or_credentials() {
        let p=Profile::new(Some(vec![123]),vec![Some("bad\nvalue".into())]);assert_eq!(p.titles,None);assert_eq!(p.sizes,Some(vec![123]));
        let value=serde_json::json!({"id":"album","songCount":1,"coverArt":"https://private/?token=secret","song":[{"id":"a","title":"One","size":123,"path":"/private/music.flac","url":"https://private/?token=secret"}]});
        let p=navidrome_profile(&value,"album").unwrap();let text=serde_json::to_string(&p).unwrap();
        assert_eq!(p.cover_id,None);assert!(!text.contains("private"));assert!(!text.contains("secret"));assert!(!text.contains("path"));
    }

    #[test] fn plex_folder_projection_supports_disc_directories_without_returning_paths() {
        for path in ["/private/music/Aja (UHQR)/01.flac", "Z:\\private\\Aja (UHQR)\\Disc 2\\01.flac", "/private/Aja (UHQR)/CD1/01.flac"] {
            assert_eq!(plex_album_folder(path),Some("Aja (UHQR)".into()));
        }
        for path in ["file.flac", "https://private/file.flac", "/private/bad\nfolder/01.flac", "/private/../01.flac"] { assert_eq!(plex_album_folder(path),None); }
        let mut value=serde_json::json!({"MediaContainer":{"librarySectionID":"7","size":2,"totalSize":2,"Metadata":[
          {"ratingKey":"1","type":"track","parentRatingKey":"42","title":"One","Media":[{"Part":[{"file":"/private/Aja (UHQR)/CD1/01.flac","size":100}]}]},
          {"ratingKey":"2","type":"track","parentRatingKey":"42","title":"Two","Media":[{"Part":[{"file":"/private/Aja (UHQR)/CD2/02.flac","size":200}]}]}]}});
        let p=plex_profile(&value,"7","42").unwrap();assert_eq!(p.edition_title,Some("Aja (UHQR)".into()));
        let text=serde_json::to_string(&p).unwrap();assert!(!text.contains("private"));assert!(!text.contains("file"));
        value["MediaContainer"]["Metadata"][1]["Media"][0]["Part"][0]["file"]=serde_json::json!("/private/Other/02.flac");
        assert_eq!(plex_profile(&value,"7","42").unwrap().edition_title,None);
    }

}
