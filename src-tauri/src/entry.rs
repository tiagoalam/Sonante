// Keep the established Linux module tree at the crate root, unchanged.
pub mod audio_route;
#[cfg(target_os = "linux")]
include!("lib.rs");

#[cfg(target_os = "windows")]
mod windows_shared;
#[cfg(target_os = "windows")]
mod windows_radio;
#[cfg(target_os = "windows")]
mod windows_radio_host;
#[cfg(target_os = "windows")]
mod windows_credentials;
#[cfg(target_os = "windows")]
mod windows_navidrome;
#[cfg(target_os = "windows")]
mod windows_plex;
#[cfg(target_os = "windows")]
#[path="windows_diagnostics.rs"]
mod diagnostics;
#[cfg(target_os = "windows")]
pub use windows_shared::{audio, navidrome, persistence, plex};
#[cfg(target_os = "windows")]
mod playlists;
#[cfg(target_os = "windows")]
mod windows_playlists;
#[cfg(target_os = "windows")]
mod windows_favorites;
#[cfg(target_os = "windows")]
mod windows_artwork_progress;
#[cfg(target_os = "windows")]
mod windows_history;
#[cfg(target_os = "windows")]
mod windows_scrobbling;
#[cfg(target_os = "windows")]
mod windows_smart_playlists;
#[cfg(target_os = "windows")]
mod windows_smart_remote;
#[cfg(target_os = "windows")]
mod windows_instance;
#[cfg(target_os = "windows")]
mod windows_sleep_timer;
#[cfg(target_os = "windows")]
mod windows_player;
#[cfg(target_os = "windows")]
mod local_file_info;
#[cfg(target_os = "windows")]
mod windows_file_info;
#[cfg(target_os = "windows")]
mod windows_io;
#[cfg(target_os = "windows")]
mod windows_source_info;
#[cfg(target_os = "windows")]
mod windows_output;
#[cfg(target_os = "windows")]
mod windows_system_media;
#[cfg(target_os = "windows")]
mod shared_mute;
#[cfg(target_os = "windows")]
mod windows_mpd;
#[cfg(target_os = "windows")]
mod windows_audio;
#[cfg(target_os = "windows")]
mod equalizer;
#[cfg(target_os = "windows")]
mod windows_dsp;
#[cfg(target_os = "windows")]
mod windows_equalizer;
#[cfg(windows)]
mod windows_setup;
#[cfg(target_os = "windows")]
use windows_dsp as shared_dsp;
#[cfg(target_os = "windows")]
#[path = "windows_dsp/portable.rs"]
mod equalizer_portable;

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::run;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("Sonante currently supports Linux and the Windows launch preview only.");

#[cfg(target_os="windows")]
mod windows_metadata;
#[cfg(target_os="windows")]
mod windows_editions;
#[cfg(target_os="windows")]
mod windows_online_artwork;
#[cfg(target_os="windows")]
#[path="windows_local_diagnostics.rs"]
mod local_diagnostics;
#[cfg(target_os="windows")]
mod online_metadata;
#[cfg(target_os="windows")]
#[path="windows_metadata/musicbrainz.rs"]
mod online_artwork;

#[cfg(windows)]
#[path="windows_remote/core.rs"]
mod remote;
#[cfg(windows)]
mod remote_http;
#[cfg(windows)]
mod windows_remote;
#[cfg(windows)]
#[path="windows_upnp/core.rs"]
mod upnp;
#[cfg(windows)]
#[path="windows_upnp/protocol.rs"]
mod upnp_protocol;
#[cfg(windows)]
mod windows_upnp;
