//! Windows source foundation: original Subsonic client and encrypted credentials.
//! No UI/IPC capability is exposed until playback, persistence and source leases
//! are connected. Linux retains its complete original implementation unchanged.
pub(crate) mod api;
pub(crate) mod store;
pub(crate) const INVALID: &str = "NAVIDROME_INVALID";
pub(crate) const AUTH: &str = "NAVIDROME_AUTH";
pub(crate) const PROTOCOL: &str = "NAVIDROME_PROTOCOL";
pub(crate) const UNAVAILABLE: &str = "NAVIDROME_UNAVAILABLE";
pub(crate) const STORAGE: &str = "NAVIDROME_STORAGE";

mod connection;
mod proxy;
mod locator;
pub(crate) const CHANGED: &str = "NAVIDROME_CONTEXT_CHANGED";
use connection::Connection;

mod catalog;
mod discovery;

mod favorites;
mod unique_json;

mod host;
pub(crate) use host::{Controller,Status,PreparedAccount};

mod playback;
mod facade;
pub(crate) use catalog::{Sort, Page, Detail, PlayerNavigation};
pub(crate) use discovery::{SearchResults, ArtistDetail, Explore};

pub(crate) fn playback_error(error:&str)->&'static str {
    match error {AUTH=>AUTH,INVALID=>INVALID,PROTOCOL=>PROTOCOL,STORAGE=>STORAGE,CHANGED=>CHANGED,_=>UNAVAILABLE}
}

mod smart;
