//! Reuse the established option state machine; this adapter only changes transport errors.
#[path = "../playback_options.rs"]
mod shared;
use super::protocol::{self, Result};
pub use shared::{PlaybackOptionAction, PlaybackOptions, SingleMode};
pub fn read(lines: &[String]) -> Result<Option<PlaybackOptions>> {
    for key in ["repeat", "single", "random"] {
        protocol::field(lines, key)?;
    }
    Ok(PlaybackOptions::from_status(lines))
}
pub fn change(
    action: PlaybackOptionAction,
    expected: PlaybackOptions,
    mut query: impl FnMut(&str) -> Result<Vec<String>>,
) -> Result<PlaybackOptions> {
    let mut failure = None;
    shared::change(action, expected, |command| {
        let result = query(command).and_then(|lines| {
            if command == "status" {
                read(&lines)?.ok_or("playback_options_unavailable")?;
            }
            Ok(lines)
        });
        result.map_err(|code| {
            failure = Some(code);
            code.to_string()
        })
    })
    .map_err(|_| failure.unwrap_or("playback_options_changed"))
}
pub fn restore(
    target: PlaybackOptions,
    mut query: impl FnMut(&str) -> Result<Vec<String>>,
) -> Result<()> {
    use shared::SingleMode;
    query(if target.repeat {
        "repeat 1"
    } else {
        "repeat 0"
    })?;
    query(match target.single {
        SingleMode::Off => "single 0",
        SingleMode::On => "single 1",
        SingleMode::Oneshot => "single oneshot",
    })?;
    query(if target.random {
        "random 1"
    } else {
        "random 0"
    })?;
    if read(&query("status")?)? != Some(target) {
        return Err("playback_options_changed");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_options_are_not_accepted() {
        let lines = ["repeat: 0", "repeat: 1", "single: 0", "random: 0"].map(String::from);
        assert_eq!(read(&lines), Err("mpd_protocol"));
    }
    #[test]
    fn restore_confirms_options_without_queue_or_audio_commands() {
        let target = PlaybackOptions::from_status(
            &["repeat: 1", "single: 1", "random: 1"].map(String::from),
        )
        .unwrap();
        let mut commands = Vec::new();
        restore(target, |command| {
            commands.push(command.to_string());
            Ok(if command == "status" {
                ["repeat: 1", "single: 1", "random: 1"]
                    .map(String::from)
                    .into()
            } else {
                Vec::new()
            })
        })
        .unwrap();
        assert_eq!(commands, ["repeat 1", "single 1", "random 1", "status"]);
        assert_eq!(restore(target, |_| Err("mpd_ack")), Err("mpd_ack"));
    }
}
