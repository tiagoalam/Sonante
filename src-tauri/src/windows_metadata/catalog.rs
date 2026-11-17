//! Original catalog DTOs used by the read-only doctor resolver.
use serde::{Serialize,Deserialize};
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalItem {
    pub item_type: String,
    pub path: String,
    pub name: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbumDisc {
    pub number: u32,
    pub label: String,
    pub folder_path: String,
    pub track_count: usize,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbumDiscTracks {
    pub disc: LocalAlbumDisc,
    pub tracks: Vec<LocalItem>,
}
