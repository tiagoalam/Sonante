//! Runtime admission for optional music sources. Credentials and stored collections stay intact.
use crate::{
    audio::{MediaLocator, TrackMetadata},
    config::{AppConfig, EnabledSources},
};
use std::sync::atomic::{AtomicU8, Ordering};
const LOCAL: u8 = 1;
const PLEX: u8 = 2;
const NAVIDROME: u8 = 4;
static ENABLED: AtomicU8 = AtomicU8::new(7);
fn mask(s: EnabledSources) -> u8 {
    u8::from(s.local) | (u8::from(s.plex) << 1) | (u8::from(s.navidrome) << 2)
}
pub(crate) fn install(config: &AppConfig) {
    ENABLED.store(mask(config.sources.unwrap_or_default()), Ordering::Release);
}
pub(crate) fn local() -> bool {
    ENABLED.load(Ordering::Acquire) & LOCAL != 0
}
pub(crate) fn plex() -> bool {
    ENABLED.load(Ordering::Acquire) & PLEX != 0
}
pub(crate) fn navidrome() -> bool {
    ENABLED.load(Ordering::Acquire) & NAVIDROME != 0
}
pub(crate) fn require_local() -> Result<(), String> {
    if local() {
        Ok(())
    } else {
        Err("SOURCE_DISABLED".into())
    }
}
pub(crate) fn allows(s: EnabledSources, locator: &MediaLocator) -> bool {
    match locator {
        MediaLocator::Local { .. } => s.local,
        MediaLocator::Plex { .. } => s.plex,
        MediaLocator::Navidrome { .. } => s.navidrome,
        _ => true,
    }
}
pub(crate) fn require_locator(locator: &MediaLocator) -> Result<(), String> {
    let allowed = match locator {
        MediaLocator::Local { .. } => local(),
        MediaLocator::Plex { .. } => plex(),
        MediaLocator::Navidrome { .. } => navidrome(),
        _ => true,
    };
    if allowed {
        Ok(())
    } else {
        Err("SOURCE_DISABLED".into())
    }
}
pub(crate) fn require_tracks(tracks: &[TrackMetadata]) -> Result<(), String> {
    for track in tracks {
        if let Some(locator) = &track.media_locator {
            require_locator(locator)?;
        } else {
            require_local()?;
        }
    }
    Ok(())
}
pub(crate) fn validate_disable(
    current: &AppConfig,
    next: &AppConfig,
    tracks: &[TrackMetadata],
) -> Result<(), String> {
    let before = current.sources.unwrap_or_default();
    let after = next.sources.unwrap_or_default();
    if before == after {
        return Ok(());
    }
    for track in tracks {
        let disabled = match &track.media_locator {
            Some(locator) => allows(before, locator) && !allows(after, locator),
            None => before.local && !after.local,
        };
        if disabled {
            return Err("SOURCE_IN_QUEUE".into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_omits_switches_and_preserves_enabled_sources() {
        let config = AppConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("sources").is_none());
        assert_eq!(mask(config.sources.unwrap_or_default()), 7);
        let mut explicit = config;
        explicit.sources = Some(EnabledSources {
            local: false,
            plex: true,
            navidrome: false,
        });
        let restored: AppConfig =
            serde_json::from_value(serde_json::to_value(&explicit).unwrap()).unwrap();
        assert_eq!(restored, explicit);
    }
    #[test]
    fn disable_refuses_only_the_affected_queue_and_keeps_external_sources() {
        let current = AppConfig::default();
        let mut next = current.clone();
        next.sources = Some(EnabledSources {
            local: false,
            plex: true,
            navidrome: true,
        });
        let mut track = TrackMetadata {
            title: "fixture".into(),
            artist: "artist".into(),
            album: "album".into(),
            thumb: None,
            plex_image: None,
            media_locator: None,
            uri: String::new(),
            duration: None,
        };
        track.media_locator = Some(MediaLocator::Local {
            uri: "fixture/song.flac".into(),
        });
        assert_eq!(
            validate_disable(&current, &next, &[track.clone()]),
            Err("SOURCE_IN_QUEUE".into())
        );
        track.media_locator = Some(MediaLocator::Navidrome {
            account_id: None,
            connection_id: "fixture".into(),
            song_id: "song".into(),
        });
        assert!(validate_disable(&current, &next, &[track]).is_ok());
        assert!(validate_disable(&current, &next, &[]).is_ok());
    }
}
