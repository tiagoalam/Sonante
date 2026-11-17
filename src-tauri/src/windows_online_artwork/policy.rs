//! Original Linux eligibility, match and image policies, no HTTP or storage.
use serde::Deserialize;
use base64::Engine;
#[derive(Clone,Deserialize)]
pub struct OnlineAlbumCoverRequest {
    pub(crate) album_id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) year: Option<String>,
}

impl OnlineAlbumCoverRequest {
    pub(crate) fn validated(mut self) -> Result<Option<Self>, String> {
        if self.album_id.len() > 2048
            || self.title.chars().count() > 512
            || self.artist.chars().count() > 512
        {
            return Err("Metadados de álbum excedem o limite para busca de capa.".into());
        }
        self.album_id = self.album_id.trim().to_string();
        self.title = self.title.trim().to_string();
        self.artist = self.artist.trim().to_string();
        self.year = self.year.take().and_then(|year| {
            let year = year.trim().to_string();
            (year.len() == 4
                && year
                    .parse::<u16>()
                    .is_ok_and(|n| (1850..=2100).contains(&n)))
            .then_some(year)
        });
        if self.album_id.is_empty()
            || self.title.is_empty()
            || self.artist.is_empty()
            || matches!(
                normalize(&self.artist).as_str(),
                "various artists" | "artista desconhecido"
            )
        {
            return Ok(None);
        }
        Ok(Some(self))
    }
}

pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub(super) fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

pub(crate) fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[derive(Deserialize)]
pub(super) struct SearchResponse {
    #[serde(rename = "release-groups")]
    release_groups: Vec<ReleaseGroup>,
}
#[derive(Deserialize)]
struct ReleaseGroup {
    id: String,
    title: String,
    score: u8,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<ArtistCredit>,
    #[serde(default, rename = "first-release-date")]
    first_release_date: Option<String>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum ArtistCredit {
    Text(String),
    Person {
        name: String,
        #[serde(default, rename = "joinphrase")]
        join_phrase: String,
    },
}

pub(crate) fn valid_mbid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub(super) fn select_release_group(
    results: &SearchResponse,
    title: &str,
    artist: &str,
    year: Option<&str>,
) -> Option<String> {
    let title = normalize(title);
    let artist = normalize(artist);
    let mut candidates: Vec<_> = results
        .release_groups
        .iter()
        .filter(|group| {
            if group.score < 90 || !valid_mbid(&group.id) || normalize(&group.title) != title {
                return false;
            }
            let credit = group
                .artist_credit
                .iter()
                .map(|item| match item {
                    ArtistCredit::Text(text) => text.clone(),
                    ArtistCredit::Person { name, join_phrase } => format!("{name}{join_phrase}"),
                })
                .collect::<String>();
            normalize(&credit) == artist
        })
        .collect();
    candidates.sort_by_key(|group| std::cmp::Reverse(group.score));
    let best_score = candidates.first()?.score;
    candidates.retain(|group| group.score == best_score);
    if candidates.len() > 1 {
        if let Some(year) = year {
            let matching: Vec<_> = candidates
                .iter()
                .filter(|group| {
                    group
                        .first_release_date
                        .as_deref()
                        .is_some_and(|date| date.starts_with(year))
                })
                .collect();
            if matching.len() == 1 {
                return Some(matching[0].id.clone());
            }
        }
        return None;
    }
    Some(candidates[0].id.clone())
}


#[cfg(test)]mod tests{use super::*;
const ID:&str="12345678-1234-1234-1234-123456789abc";
    fn request() -> OnlineAlbumCoverRequest {
        OnlineAlbumCoverRequest {
            album_id: "root|album".into(),
            title: " Album ".into(),
            artist: " Artist ".into(),
            year: None,
        }
    }
    fn group(score: u8, title: &str, artist: &str) -> ReleaseGroup {
        ReleaseGroup {
            id: ID.into(),
            score,
            title: title.into(),
            artist_credit: vec![ArtistCredit::Person {
                name: artist.into(),
                join_phrase: String::new(),
            }],
            first_release_date: None,
        }
    }
#[test]
    fn invalid_input_skips_lookup() {
        assert!(OnlineAlbumCoverRequest {
            title: " ".into(),
            ..request()
        }
        .validated()
        .unwrap()
        .is_none());
        assert!(OnlineAlbumCoverRequest {
            artist: "Various Artists".into(),
            ..request()
        }
        .validated()
        .unwrap()
        .is_none());
        assert!(OnlineAlbumCoverRequest {
            artist: "Artista Desconhecido".into(),
            ..request()
        }
        .validated()
        .unwrap()
        .is_none());
    }
#[test]
    fn matching_is_conservative() {
        let response = |groups| SearchResponse {
            release_groups: groups,
        };
        assert_eq!(
            select_release_group(
                &response(vec![group(95, "  album  ", "artist")]),
                "Album",
                "Artist",
                None
            ),
            Some(ID.into())
        );
        assert!(select_release_group(
            &response(vec![group(89, "Album", "Artist")]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        assert!(select_release_group(
            &response(vec![group(95, "Other", "Artist")]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        assert!(select_release_group(
            &response(vec![group(95, "Album", "Other")]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        assert!(select_release_group(
            &response(vec![
                group(95, "Album", "Artist"),
                group(95, "Album", "Artist")
            ]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        let mut first = group(95, "Album", "Artist");
        first.first_release_date = Some("2001-01-01".into());
        let mut second = group(95, "Album", "Artist");
        second.id = "87654321-1234-1234-1234-123456789abc".into();
        second.first_release_date = Some("2002".into());
        assert_eq!(
            select_release_group(
                &response(vec![first, second]),
                "Album",
                "Artist",
                Some("2002")
            ),
            Some("87654321-1234-1234-1234-123456789abc".into())
        );
    }
#[test]
    fn image_formats_require_magic() {
        assert_eq!(image_mime(&[0xff, 0xd8, 0xff]), Some("image/jpeg"));
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
        assert_eq!(image_mime(b"RIFF1234WEBP"), Some("image/webp"));
        assert_eq!(image_mime(b"<svg/>"), None);
    }
}
