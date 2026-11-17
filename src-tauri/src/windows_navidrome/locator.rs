//! Account-bound references reuse the shared MediaLocator wire schema.
use super::{
    api,
    connection::{Connection, Registry},
    CHANGED, INVALID, PROTOCOL,
};
use crate::audio::MediaLocator;
use std::sync::Arc;

fn bound(connection: &Connection, locator: &MediaLocator) -> Result<MediaLocator, String> {
    let MediaLocator::Navidrome {
        connection_id,
        account_id,
        song_id,
    } = locator
    else {
        return Err(INVALID.into());
    };
    if !connection.active()
        || !api::id(song_id)
        || match account_id {
            Some(account) => {
                !crate::navidrome::valid_account(account) || *account != connection.account()
            }
            None => *connection_id != connection.credentials.connection_id,
        }
    {
        return Err(CHANGED.into());
    }
    Ok(MediaLocator::Navidrome {
        connection_id: connection.credentials.connection_id.clone(),
        account_id: Some(connection.account()),
        song_id: song_id.clone(),
    })
}
pub(crate) struct Resolved {
    connection: Arc<Connection>,
    pub(crate) uri: String,
    pub(crate) locator: MediaLocator,
    pub(super) duration: Option<f64>,
}
impl Resolved {
    pub(super) fn health_signal(&self)->std::sync::Arc<std::sync::atomic::AtomicBool>{self.connection.health_signal()}
    pub(super) fn active(&self)->bool { self.connection.active() }
    pub(super) fn prepared(connection:Arc<Connection>, locator:MediaLocator, uri:String, duration:Option<f64>)->Result<Self,String> {
        if bound(&connection,&locator)?!=locator {return Err(CHANGED.into());}
        let MediaLocator::Navidrome{song_id,..}=&locator else{return Err(INVALID.into())};
        if connection.stream(song_id)?!=uri {return Err(CHANGED.into());}
        Ok(Self{connection,locator,uri,duration:crate::windows_mpd::library::presentation_duration(duration)})
    }
}
impl Registry {
    pub(crate) async fn resolve_saved(&self, locator: &MediaLocator) -> Result<Resolved, String> {
        let saved = crate::navidrome::storage_locator(locator).ok_or(INVALID)?;
        let MediaLocator::Navidrome {
            account_id: Some(account),
            song_id,
            ..
        } = &saved
        else {
            return Err(INVALID.into());
        };
        let c = self.for_account(account)?;
        let root = c.request("getSong", &[("id", song_id.clone())]).await?;
        let song = root.get("song")
            .filter(|v| v["id"].as_str() == Some(song_id.as_str()))
            .ok_or(PROTOCOL)?;
        // A reconnect between HTTP completion and publication cannot lend a new URL.
        self.with_current(&c, || {
            Ok(Resolved {
                uri: c.stream(song_id)?,
                locator: bound(&c, &saved)?,
                connection: c.clone(),
                duration: song["duration"].as_f64().filter(|v| v.is_finite() && *v > 0.0 && *v <= 86400.0),
            })
        })
    }
    pub(crate) fn with_resolved<T>(
        &self,
        tracks: &[Resolved],
        effect: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let Some(first) = tracks.first() else {
            return Err(INVALID.into());
        };
        self.with_current(&first.connection, || {
            for track in tracks {
                if !Arc::ptr_eq(&first.connection, &track.connection)
                    || bound(&first.connection, &track.locator)? != track.locator
                {
                    return Err(CHANGED.into());
                }
                let MediaLocator::Navidrome { song_id, .. } = &track.locator else {
                    return Err(INVALID.into());
                };
                if first.connection.stream(song_id)? != track.uri {
                    return Err(CHANGED.into());
                }
            }
            effect()
        })
    }
}
#[cfg(test)]
#[path = "locator_tests.rs"]
mod tests;
