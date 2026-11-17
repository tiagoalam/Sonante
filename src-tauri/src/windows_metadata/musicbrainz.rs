//! Windows platform shim used by unchanged online_metadata.rs, no artwork cache.
use std::{sync::{Mutex,OnceLock},time::{Duration,Instant}};
pub(crate) const USER_AGENT: &str = concat!(
    "Sonante/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/tiagoalam/Sonante)"
);
const MIN_REQUEST_INTERVAL:Duration=Duration::from_secs(1);
static LAST_MUSICBRAINZ_REQUEST:OnceLock<Mutex<Option<Instant>>>=OnceLock::new();
fn next_request_at(last: Option<Instant>, now: Instant) -> Instant {
    last.and_then(|last| last.checked_add(MIN_REQUEST_INTERVAL))
        .map_or(now, |next| next.max(now))
}
pub(crate) fn wait_for_musicbrainz()->Result<(),String>{
 let limiter=LAST_MUSICBRAINZ_REQUEST.get_or_init(||Mutex::new(None));
 let mut last=limiter.lock().map_err(|_|"Limitador MusicBrainz indisponível.".to_string())?;
 let next=next_request_at(*last,Instant::now());
 if let Some(wait)=next.checked_duration_since(Instant::now()){std::thread::sleep(wait);}
 *last=Some(Instant::now());Ok(())
}
