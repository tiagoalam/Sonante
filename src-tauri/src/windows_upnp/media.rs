//! Volatile receiver media. The URL is an in-memory capability, never a catalog
//! locator or journal entry; deserializing presentation cannot recreate it.
use crate::windows_mpd::{
    library::{Selection, Track},
    protocol::Result,
    transport::MediaLease,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playing {
    pub session_id: String,
    pub artwork: Option<String>,
    #[serde(skip)]
    lease: Option<Arc<Lease>>,
}
pub(crate) struct Lease {
    id: String,
    uri: String,
    live: Arc<AtomicBool>,
}
impl Lease {
    pub(crate) fn healthy(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
    pub(crate) fn signal(&self) -> Arc<AtomicBool> {
        self.live.clone()
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub(crate) fn selection(media: crate::upnp::Media) -> Result<Selection> {
    if !valid_id(&media.id) || crate::upnp_protocol::media_url(&media.uri).is_err() {
        return Err("UPNP_MEDIA_INVALID");
    }
    if [&media.title, &media.artist, &media.album]
        .iter()
        .any(|s| s.chars().count() > 512 || s.chars().any(char::is_control))
    {
        return Err("UPNP_MEDIA_INVALID");
    }
    let artwork = media
        .artwork
        .filter(|url| crate::upnp_protocol::media_url(url).is_ok());
    let lease = Arc::new(Lease {
        id: media.id.clone(),
        uri: media.uri,
        live: Arc::new(AtomicBool::new(true)),
    });
    Ok(Selection {
        root: Default::default(),
        start: 0,
        album_id: None,
        tracks: vec![Track {
            id: media.id.clone(),
            title: media.title,
            artist: media.artist,
            album: media.album,
            disc: 1,
            number: 1,
            file_name: String::new(),
            catalog_album_id: None,
            path: Default::default(),
            media_root: None,
            radio: None,
            upnp: Some(Playing {
                session_id: media.id,
                artwork,
                lease: Some(lease),
            }),
            remote_source: None,
            remote_connection_id: None,
            plex_image: None,
            remote_lease: None,
        }],
    })
}

/// Qualification-only UPnP-shaped selection for the private loopback PCM
/// fixture. This is absent from product builds and accepts only its exact
/// synthetic endpoint shape; production `selection` keeps the normal URL
/// policy unchanged.
#[cfg(test)]
pub(crate) fn qualification_selection(uri: String) -> Result<Selection> {
    let url = reqwest::Url::parse(&uri).map_err(|_| "UPNP_MEDIA_INVALID")?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.port().is_some_and(|port| port != 0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/own.wav"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("UPNP_MEDIA_INVALID");
    }
    let id = "12345678-1234-4234-8234-123456789abc";
    let lease = Arc::new(Lease {
        id: id.into(),
        uri,
        live: Arc::new(AtomicBool::new(true)),
    });
    Ok(Selection {
        root: Default::default(),
        start: 0,
        album_id: None,
        tracks: vec![Track {
            id: id.into(),
            title: "Own synthetic WAV".into(),
            artist: "Qualification".into(),
            album: "Qualification".into(),
            disc: 1,
            number: 1,
            file_name: String::new(),
            catalog_album_id: None,
            path: Default::default(),
            media_root: None,
            radio: None,
            upnp: Some(Playing {
                session_id: id.into(),
                artwork: None,
                lease: Some(lease),
            }),
            remote_source: None,
            remote_connection_id: None,
            plex_image: None,
            remote_lease: None,
        }],
    })
}
pub(crate) fn admit(selected: &Selection) -> Result<(Vec<MediaLease>, Vec<String>)> {
    if selected.start != 0
        || selected.tracks.len() != 1
        || !selected.root.as_os_str().is_empty()
        || selected.album_id.is_some()
    {
        return Err("UPNP_MEDIA_INVALID");
    }
    let track = &selected.tracks[0];
    let item = track.upnp.as_ref().ok_or("UPNP_SESSION_EXPIRED")?;
    if track.radio.is_some()
        || track.remote_source.is_some()
        || track.media_root.is_some()
        || !track.path.as_os_str().is_empty()
    {
        return Err("UPNP_MEDIA_INVALID");
    }
    let lease = item
        .lease
        .clone()
        .filter(|l| l.healthy() && l.id == item.session_id && track.id == l.id)
        .ok_or("UPNP_SESSION_EXPIRED")?;
    Ok((
        vec![MediaLease::Upnp(lease.clone())],
        vec![lease.uri.clone()],
    ))
}
pub(crate) fn require(tracks: &[Track], expected: &str) -> Result<()> {
    if tracks.len() == 1
        && tracks[0].upnp.as_ref().is_some_and(|s| {
            s.session_id == expected && s.lease.as_ref().is_some_and(|l| l.healthy())
        })
    {
        Ok(())
    } else {
        Err("UPNP_SESSION_CHANGED")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn copy(s: &Selection) -> Selection {
        Selection {
            root: s.root.clone(),
            tracks: s.tracks.clone(),
            start: s.start,
            album_id: s.album_id.clone(),
        }
    }
    fn selected() -> Selection {
        selection(crate::upnp::Media {
            id: "12345678-1234-4234-8234-123456789abc".into(),
            uri: "http://192.168.25.9/own.flac?key=PRIVATE_CAPABILITY".into(),
            metadata: String::new(),
            title: "Own synthetic".into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            artwork: None,
        })
        .unwrap()
    }
    #[test]
    fn qualification_selection_accepts_only_the_owned_loopback_fixture_uri() {
        let selected = qualification_selection("http://127.0.0.1:41000/own.wav".into()).unwrap();
        let (_, uris) = admit(&selected).unwrap();
        assert_eq!(uris, vec!["http://127.0.0.1:41000/own.wav".to_owned()]);
        for uri in [
            "http://192.168.1.10:41000/own.wav",
            "http://127.0.0.1/own.wav",
            "http://127.0.0.1:41000/other.wav",
            "http://user@127.0.0.1:41000/own.wav",
            "http://127.0.0.1:41000/own.wav?token=x",
        ] {
            assert!(qualification_selection(uri.into()).is_err(), "{uri}");
        }
    }
    #[test]
    fn production_selection_continues_to_reject_loopback_media() {
        let media = crate::upnp::Media {
            id: "12345678-1234-4234-8234-123456789abc".into(),
            uri: "http://127.0.0.1:41000/own.wav".into(),
            metadata: String::new(),
            title: "Own synthetic".into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            artwork: None,
        };
        assert!(matches!(selection(media), Err("UPNP_MEDIA_INVALID")));
    }
    #[test]
    fn serialization_never_contains_the_media_uri_and_cannot_restore_authority() {
        let s = selected();
        let (_, uris) = admit(&s).unwrap();
        assert!(uris[0].contains("PRIVATE_CAPABILITY"));
        let value = serde_json::to_string(&s.tracks[0]).unwrap();
        assert!(!value.contains("PRIVATE_CAPABILITY"));
        assert!(!value.contains("192.168"));
        let row: Track = serde_json::from_str(&value).unwrap();
        let fake = Selection {
            tracks: vec![row],
            ..s
        };
        assert!(matches!(admit(&fake), Err("UPNP_SESSION_EXPIRED")));
    }
    #[test]
    fn mixed_or_mutated_receivers_are_rejected_before_transport_and_stale_sessions_cannot_control_another_queue(
    ) {
        let s = selected();
        let id = s.tracks[0].id.clone();
        require(&s.tracks, &id).unwrap();
        assert_eq!(require(&s.tracks, "other"), Err("UPNP_SESSION_CHANGED"));
        let mut other = copy(&s);
        other.tracks[0].path = "arbitrary-file".into();
        assert!(admit(&other).is_err());
        other = copy(&s);
        other.tracks.push(other.tracks[0].clone());
        assert!(admit(&other).is_err());
        other = s;
        other.tracks[0].upnp.as_mut().unwrap().session_id = "changed".into();
        assert!(admit(&other).is_err());
    }
    #[test]
    fn replaced_generation_does_not_revoke_a_new_candidate_sharing_the_media() {
        let s = selected();
        let (leases, _) = admit(&s).unwrap();
        let flag = leases[0].health_signal().unwrap();
        let candidate = copy(&s);
        drop(leases);
        drop(s);
        assert!(admit(&candidate).is_ok());
        let (leases, _) = admit(&candidate).unwrap();
        assert!(leases[0].healthy());
        drop(candidate);
        assert!(leases[0].healthy());
        drop(leases);
        let watch = crate::windows_mpd::liveness::Watch::new(vec![flag]);
        assert!(watch.is_err());
    }
    #[test]
    fn durable_queue_projection_refuses_receiver_media_instead_of_encoding_it_as_local() {
        let s = selected();
        assert!(crate::windows_mpd::queue_v3::Entry::local(
            &s.tracks[0],
            std::path::Path::new("/own-fixture")
        )
        .is_err());
    }
}
