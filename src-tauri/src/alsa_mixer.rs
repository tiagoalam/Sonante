use alsa::mixer::{Mixer, Selem, SelemChannelId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlaybackVolumeControl {
    pub name: String,
    pub index: u32,
    pub channels: Vec<String>,
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MixerSelection {
    Software,
    Hardware {
        mixer_device: String,
        control: PlaybackVolumeControl,
    },
}

pub(crate) fn select_mixer(
    mixer_device: String,
    controls: Vec<PlaybackVolumeControl>,
) -> MixerSelection {
    match controls.as_slice() {
        [control] => MixerSelection::Hardware {
            mixer_device,
            control: control.clone(),
        },
        _ => MixerSelection::Software,
    }
}

pub(crate) fn detect_for_pcm(pcm_device: &str) -> MixerSelection {
    let Some(mixer_device) = mixer_device_for_pcm(pcm_device) else {
        eprintln!(
            "[Supervisor] Dispositivo ALSA Direct sem associação segura a uma placa de mixer; usando volume por software."
        );
        return MixerSelection::Software;
    };

    let controls = match enumerate_playback_volume_controls(&mixer_device) {
        Ok(controls) => controls,
        Err(error) => {
            eprintln!(
                "[Supervisor] Não foi possível sondar o mixer ALSA de {}: {}. Usando volume por software.",
                mixer_device, error
            );
            return MixerSelection::Software;
        }
    };

    match controls.as_slice() {
        [] => {
            println!(
                "[Supervisor] Nenhum controle ALSA de volume de reprodução utilizável em {}; usando volume por software.",
                mixer_device
            );
        }
        [control] => {
            println!(
                "[Supervisor] Mixer ALSA de hardware selecionado: dispositivo={}, controle={}#{}, canais={}, faixa={}..{}.",
                mixer_device,
                control.name,
                control.index,
                control.channels.join(","),
                control.min,
                control.max
            );
        }
        _ => {
            let descriptions = controls
                .iter()
                .map(|control| {
                    format!(
                        "{}#{} [{}] {}..{}",
                        control.name,
                        control.index,
                        control.channels.join(","),
                        control.min,
                        control.max
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            eprintln!(
                "[Supervisor] Mixer ALSA ambíguo em {} ({} controles: {}); seleção explícita será necessária. Usando volume por software.",
                mixer_device,
                controls.len(),
                descriptions
            );
        }
    }

    select_mixer(mixer_device, controls)
}

fn mixer_device_for_pcm(pcm_device: &str) -> Option<String> {
    let hardware = pcm_device.strip_prefix("hw:")?;
    let card = hardware.split(',').next()?.trim();
    if card.is_empty() {
        return None;
    }

    if let Some(card_value) = card.strip_prefix("CARD=") {
        if card_value.is_empty() {
            None
        } else {
            Some(format!("hw:CARD={}", card_value))
        }
    } else if card.contains('=') {
        None
    } else {
        Some(format!("hw:{}", card))
    }
}

fn enumerate_playback_volume_controls(
    mixer_device: &str,
) -> Result<Vec<PlaybackVolumeControl>, String> {
    let mixer = Mixer::new(mixer_device, false).map_err(|error| error.to_string())?;
    let mut controls = Vec::new();

    for element in mixer.iter() {
        let Some(selem) = Selem::new(element) else {
            continue;
        };
        if !selem.has_playback_volume() {
            continue;
        }

        let (min, max) = selem.get_playback_volume_range();
        if min >= max {
            continue;
        }

        let channels = SelemChannelId::all()
            .iter()
            .copied()
            .filter(|channel| {
                selem.has_playback_channel(*channel) && selem.get_playback_volume(*channel).is_ok()
            })
            .filter_map(|channel| Selem::channel_name(channel).ok().map(str::to_string))
            .collect::<Vec<_>>();
        if channels.is_empty() {
            continue;
        }

        let id = selem.get_id();
        let name = id
            .get_name()
            .map_err(|error| format!("controle ALSA com nome inválido: {}", error))?
            .to_string();
        controls.push(PlaybackVolumeControl {
            name,
            index: id.get_index(),
            channels,
            min,
            max,
        });
    }

    controls.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.index.cmp(&right.index))
    });
    Ok(controls)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(name: &str, index: u32) -> PlaybackVolumeControl {
        PlaybackVolumeControl {
            name: name.to_string(),
            index,
            channels: vec!["Front Left".to_string(), "Front Right".to_string()],
            min: 0,
            max: 100,
        }
    }

    #[test]
    fn derives_mixer_card_from_named_and_numeric_pcm_devices() {
        assert_eq!(
            mixer_device_for_pcm("hw:CARD=SoundBar,DEV=0"),
            Some("hw:CARD=SoundBar".to_string())
        );
        assert_eq!(mixer_device_for_pcm("hw:2,1"), Some("hw:2".to_string()));
        assert_eq!(mixer_device_for_pcm("default"), None);
        assert_eq!(mixer_device_for_pcm("plughw:CARD=SoundBar,DEV=0"), None);
    }

    #[test]
    fn no_usable_control_selects_software_mixer() {
        assert_eq!(
            select_mixer("hw:CARD=NoMixer".to_string(), Vec::new()),
            MixerSelection::Software
        );
    }

    #[test]
    fn one_usable_control_selects_hardware_mixer_with_identity() {
        let candidate = control("USB Playback", 2);
        assert_eq!(
            select_mixer("hw:CARD=SoundBar".to_string(), vec![candidate.clone()]),
            MixerSelection::Hardware {
                mixer_device: "hw:CARD=SoundBar".to_string(),
                control: candidate,
            }
        );
    }

    #[test]
    fn multiple_plausible_controls_remain_software() {
        assert_eq!(
            select_mixer(
                "hw:CARD=Ambiguous".to_string(),
                vec![control("Output A", 0), control("Output B", 0)]
            ),
            MixerSelection::Software
        );
    }
}
