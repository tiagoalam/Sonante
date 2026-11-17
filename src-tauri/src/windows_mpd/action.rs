//! Existing Windows playback action wire contract, shared by generation admission.
use serde::Deserialize;
use super::options::{PlaybackOptionAction,PlaybackOptions};
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Toggle,
    SetPlaying {
        want_play: bool,
    },
    Stop,
    Release,
    Next,
    Previous,
    Jump {
        index: usize,
    },
    Seek {
        seconds: f64,
        #[serde(default)]
        index: Option<usize>,
    },
    Volume {
        value: u8,
    },
    Mute {
        muted: bool,
        token: String,
    },
    DirectMute {muted:bool,token:String},
    ChangeOptions {
        action: PlaybackOptionAction,
        expected: PlaybackOptions,
    },
}
