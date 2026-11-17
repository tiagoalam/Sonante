//! Compatibility types and pure validators required by the unchanged shared
//! PlaylistStore. This is a schema adapter, not a second audio engine or a
//! remote service implementation. Linux keeps its original module tree.
pub mod audio {
    use super::plex::PlexImageRef;
    #[derive(Clone)]
    pub(crate) struct EqTransitionWatch {
        pub intent:std::sync::Arc<std::sync::atomic::AtomicU64>,pub intent_value:u64,
        pub intervention:std::sync::Arc<std::sync::atomic::AtomicU64>,pub intervention_value:u64,
        pub stopping:std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl EqTransitionWatch {
        pub(crate) fn current(&self)->bool{
            use std::sync::atomic::Ordering;
            !self.stopping.load(Ordering::Acquire)&&self.intent.load(Ordering::Acquire)==self.intent_value&&self.intervention.load(Ordering::Acquire)==self.intervention_value
        }
    }
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plex_image: Option<PlexImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_locator: Option<MediaLocator>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uri: String,
    #[serde(default)]
    pub duration: Option<f64>,
}
    use serde::{Deserialize, Serialize};
    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum MediaLocator {
        Navidrome {
            connection_id: String,
            song_id: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            account_id: Option<String>,
        },
        Radio {
            station_id: String,
        },
        Upnp {
            session_id: String,
        },
        Local {
            uri: String,
        },
        Plex {
            server_id: String,
            part_key: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            rating_key: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            file_path: Option<String>,
        },
    }
}
pub mod plex {
    use serde::{Serialize, Deserialize};
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PlexImageRef {
    pub server_id: String,
    pub path: String,
}
    pub(crate) fn contains_plex_token(value: &str) -> bool {
        let lower = value.to_ascii_lowercase();
        lower.contains("x-plex-token")
            || urlencoding::decode(value)
                .map(|decoded| decoded.to_ascii_lowercase().contains("x-plex-token"))
                .unwrap_or(false)
    }
}
pub mod navidrome {
    use super::audio::MediaLocator;
    pub(crate) fn valid_account(id: &str) -> bool {
        id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    pub fn id(value: &str) -> bool {
        !value.is_empty()
            && !matches!(value, "." | "..")
            && value.len() <= 192
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    }
    pub(crate) fn storage_locator(locator: &MediaLocator) -> Option<MediaLocator> {
        let MediaLocator::Navidrome {
            account_id: Some(account_id),
            song_id,
            ..
        } = locator
        else {
            return None;
        };
        (valid_account(account_id) && id(song_id)).then(|| MediaLocator::Navidrome {
            account_id: Some(account_id.clone()),
            song_id: song_id.clone(),
            connection_id: String::new(),
        })
    }
}
#[path = "windows_persistence.rs"]
pub mod persistence;

// Same offline sample decoder and PCM wire as Linux, with Windows source adapters.
#[path="ab_comparison.rs"]
pub(crate) mod ab_comparison;
#[cfg(test)]
pub(crate) static AB_TEST_SERIAL:std::sync::Mutex<()>=std::sync::Mutex::new(());

#[path="windows_ab/spool.rs"]
pub(crate) mod ab_spool;
#[path="windows_ab/lifecycle.rs"]
pub(crate) mod ab_lifecycle;
#[path="windows_ab/pause.rs"]
pub(crate) mod ab_pause;
#[path="windows_analyzer/model.rs"]
pub(crate) mod analyzer;
#[cfg(windows)]
#[path="windows_analyzer/socket.rs"]
pub(crate) mod analyzer_socket;
