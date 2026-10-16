use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SingleMode {
    Off,
    On,
    Oneshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackOptions {
    pub repeat: bool,
    pub single: SingleMode,
    pub random: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackOptionAction {
    CycleRepeat,
    ToggleRandom,
}

impl PlaybackOptions {
    // Missing or unfamiliar options are unavailable, never presumed off.
    pub fn from_status(lines: &[String]) -> Option<Self> {
        let value = |key: &str| {
            lines
                .iter()
                .filter_map(|s| s.split_once(": "))
                .filter(|(k, _)| *k == key)
                .map(|(_, v)| v)
                .last()
        };
        let boolean = |key| match value(key)? {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
        Some(Self {
            repeat: boolean("repeat")?,
            single: match value("single")? {
                "0" => SingleMode::Off,
                "1" => SingleMode::On,
                "oneshot" => SingleMode::Oneshot,
                _ => return None,
            },
            random: boolean("random")?,
        })
    }
}

pub fn change(
    action: PlaybackOptionAction,
    expected: PlaybackOptions,
    mut query: impl FnMut(&str) -> Result<Vec<String>, String>,
) -> Result<PlaybackOptions, String> {
    let current = PlaybackOptions::from_status(&query("status")?)
        .ok_or("Opções de reprodução indisponíveis no MPD.")?;
    if current != expected {
        return Err("Opções de reprodução mudaram; aguarde a atualização do MPD.".into());
    }
    let mut target = current;
    match action {
        PlaybackOptionAction::ToggleRandom => target.random = !current.random,
        PlaybackOptionAction::CycleRepeat => {
            (target.repeat, target.single) = match (current.repeat, current.single) {
                (false, SingleMode::Off) => (true, SingleMode::Off),
                (true, SingleMode::Off) => (true, SingleMode::On),
                _ => (false, SingleMode::Off),
            };
        }
    }
    // Stop mutations at the first ACK failure, then inspect actual MPD state.
    // Normal snapshots expose partial changes; never rollback or retry transport.
    let applied: Result<(), String> = (|| {
        if target.repeat != current.repeat {
            query(if target.repeat {
                "repeat 1"
            } else {
                "repeat 0"
            })?;
        }
        if target.single != current.single {
            query(if target.single == SingleMode::On {
                "single 1"
            } else {
                "single 0"
            })?;
        }
        if target.random != current.random {
            query(if target.random {
                "random 1"
            } else {
                "random 0"
            })?;
        }
        Ok(())
    })();
    let observed = query("status").and_then(|lines| {
        PlaybackOptions::from_status(&lines)
            .ok_or_else(|| "Não foi possível confirmar as opções de reprodução.".to_string())
    });
    let confirmed = match (applied, observed) {
        (Err(error), Ok(_)) => return Err(error),
        (Err(error), Err(confirmation)) => {
            return Err(format!("{error}; confirmação: {confirmation}"))
        }
        (Ok(()), Err(error)) => return Err(error),
        (Ok(()), Ok(options)) => options,
    };
    if confirmed != target {
        return Err("O MPD não confirmou as opções de reprodução solicitadas.".into());
    }
    Ok(confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lines(options: PlaybackOptions) -> Vec<String> {
        vec![
            format!("repeat: {}", u8::from(options.repeat)),
            format!(
                "single: {}",
                match options.single {
                    SingleMode::Off => "0",
                    SingleMode::On => "1",
                    SingleMode::Oneshot => "oneshot",
                }
            ),
            format!("random: {}", u8::from(options.random)),
        ]
    }
    fn off() -> PlaybackOptions {
        PlaybackOptions {
            repeat: false,
            single: SingleMode::Off,
            random: false,
        }
    }

    #[test]
    fn playback_options_cycle_and_random_only_issue_native_option_commands() {
        let mut live = off();
        let mut calls = Vec::new();
        for action in [
            PlaybackOptionAction::CycleRepeat,
            PlaybackOptionAction::ToggleRandom,
            PlaybackOptionAction::CycleRepeat,
            PlaybackOptionAction::CycleRepeat,
            PlaybackOptionAction::ToggleRandom,
        ] {
            let expected = live;
            let confirmed = change(action, expected, |command| {
                calls.push(command.to_string());
                match command {
                    "status" => return Ok(lines(live)),
                    "repeat 1" => live.repeat = true,
                    "repeat 0" => live.repeat = false,
                    "single 1" => live.single = SingleMode::On,
                    "single 0" => live.single = SingleMode::Off,
                    "random 1" => live.random = true,
                    "random 0" => live.random = false,
                    _ => panic!("unexpected transport/queue command"),
                }
                Ok(vec![])
            })
            .unwrap();
            assert_eq!(confirmed, live);
        }
        assert_eq!(live, off());
        assert_eq!(
            calls
                .iter()
                .filter(|s| *s != "status")
                .cloned()
                .collect::<Vec<_>>(),
            ["repeat 1", "random 1", "single 1", "repeat 0", "single 0", "random 0"]
        );
    }

    #[test]
    fn playback_options_partial_ack_keeps_actual_state_and_stops_without_rollback() {
        let initial = PlaybackOptions {
            repeat: true,
            single: SingleMode::On,
            random: true,
        };
        let mut live = initial;
        let mut calls = vec![];
        let error = change(PlaybackOptionAction::CycleRepeat, initial, |command| {
            calls.push(command.to_string());
            match command {
                "status" => Ok(lines(live)),
                "repeat 0" => {
                    live.repeat = false;
                    Ok(vec![])
                }
                "single 0" => Err("ACK refused".into()),
                _ => panic!("unexpected retry/transport"),
            }
        })
        .unwrap_err();
        assert_eq!(error, "ACK refused");
        assert_eq!(calls, ["status", "repeat 0", "single 0", "status"]);
        assert_eq!(PlaybackOptions::from_status(&lines(live)), Some(live));
        assert_eq!(live.single, SingleMode::On);
        assert!(live.random);
    }

    #[test]
    fn playback_options_stale_unknown_and_unconfirmed_states_are_rejected() {
        let mut stale = off();
        stale.random = true;
        let mut calls = 0;
        assert!(change(PlaybackOptionAction::ToggleRandom, off(), |_| {
            calls += 1;
            Ok(lines(stale))
        })
        .is_err());
        assert_eq!(calls, 1);
        for unknown in ["", "repeat: 2", "single: future"] {
            assert!(PlaybackOptions::from_status(&[unknown.into()]).is_none());
        }
        // Success ACK alone is insufficient, as is an unreadable confirmation.
        assert!(
            change(PlaybackOptionAction::ToggleRandom, off(), |_| Ok(lines(
                off()
            )))
            .is_err()
        );
        let mut n = 0;
        assert!(change(PlaybackOptionAction::ToggleRandom, off(), |_| {
            n += 1;
            if n == 3 {
                Err("timeout".into())
            } else {
                Ok(lines(off()))
            }
        })
        .is_err());
        assert_eq!(n, 3);
    }

    #[test]
    fn playback_options_oneshot_is_represented_and_normalized_only_on_explicit_repeat() {
        let custom = PlaybackOptions {
            repeat: false,
            single: SingleMode::Oneshot,
            random: false,
        };
        assert_eq!(PlaybackOptions::from_status(&lines(custom)), Some(custom));
        let mut live = custom;
        let result = change(PlaybackOptionAction::ToggleRandom, custom, |command| {
            if command == "random 1" {
                live.random = true;
            } else {
                assert_eq!(command, "status");
            }
            Ok(lines(live))
        })
        .unwrap();
        assert_eq!(result.single, SingleMode::Oneshot);
    }
}
