//! Adapt the original discovery module by supplying its Windows client/types.
//! Its parsing, contracts, pagination and tests are compiled from the same file
//! Linux uses. No parallel discovery implementation is maintained here.
use super::*;
use crate::audio::MediaLocator;
use super::content::{PlexClient,PlexTrack};
use super::catalog::PlexAlbum;
#[path="../plex_discover.rs"]
pub(crate) mod discover;

#[cfg(test)]
mod tests {
 use super::*;
 pub(super) fn media_client_for_test(settings:&AppConfig)->PlexClient {
  // Pure parser tests use only the client metadata projection; this address is
  // never requested. Any accidental request cannot reach a personal server.
  let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
  PlexClient::from_manager(Arc::new(PlexConnectionManager::with_clients(settings,http.clone(),http,"http://127.0.0.1:9/unused".into())))
 }
}
