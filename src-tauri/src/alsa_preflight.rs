//! Momentary Direct PCM availability check, not a reservation or a playback test.
//! Success does not validate formats/DoP or guarantee that MPD can open it later.

use alsa::{pcm::PCM, Direction};
use serde::Serialize;
use std::ffi::{CStr, CString};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreflightError {
    Busy,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Availability {
    Available,
    Busy,
    Indeterminate,
    CurrentSonante,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeviceAvailability {
    pub id: String,
    pub state: Availability,
}

// Recognize hardware addresses only. Numeric and symbolic card names may alias;
// without resolving that identity, conservatively avoid probing a possible owner.
fn physical_address(device: &str) -> Option<(&str, u32)> {
    let mut parts = device.strip_prefix("hw:")?.split(',');
    let card = parts.next()?;
    let card = card.strip_prefix("CARD=").unwrap_or(card);
    if card.is_empty()
        || !card
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return None;
    }
    let device = parts.next().unwrap_or("0");
    let device = device.strip_prefix("DEV=").unwrap_or(device);
    if device.is_empty() || !device.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if card.bytes().all(|c| c.is_ascii_digit()) && card.parse::<u32>().is_err() {
        return None;
    }
    let device = device.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((card, device))
}

#[derive(PartialEq, Eq)]
enum ProbeDisposition {
    Probe,
    Current,
    Unresolved,
}

fn disposition(target: &str, current_direct: Option<&str>) -> ProbeDisposition {
    if current_direct == Some(target) {
        return ProbeDisposition::Current;
    }
    let Some((card, device)) = physical_address(target) else {
        return ProbeDisposition::Unresolved;
    };
    if let Some(current) = current_direct {
        let Some((current_card, current_device)) = physical_address(current) else {
            return ProbeDisposition::Unresolved;
        };
        let numeric = card.bytes().all(|c| c.is_ascii_digit());
        let current_numeric = current_card.bytes().all(|c| c.is_ascii_digit());
        if numeric != current_numeric {
            // Resolving a numeric index against a card ID would need additional
            // system identity data. Never open a possibly equivalent endpoint.
            return ProbeDisposition::Unresolved;
        }
        let same_card = if numeric {
            card.parse::<u32>().ok() == current_card.parse::<u32>().ok()
        } else {
            card == current_card
        };
        if same_card && device == current_device {
            return ProbeDisposition::Current;
        }
    }
    ProbeDisposition::Probe
}

pub(crate) fn should_probe(target: &str, current_direct: Option<&str>) -> bool {
    disposition(target, current_direct) == ProbeDisposition::Probe
}

pub(crate) fn availability_for_with(
    target: &str,
    enumerated: &[String],
    current_direct: Option<&str>,
    probe: impl FnMut(&str) -> Result<(), PreflightError>,
) -> Result<Vec<DeviceAvailability>, &'static str> {
    // An absent saved endpoint may be reported as owned, but never opened.
    // Every other caller-provided name must be a physical enumerated device.
    if current_direct != Some(target)
        && (physical_address(target).is_none() || !enumerated.iter().any(|id| id == target))
    {
        return Err("ALSA_AVAILABILITY_UNCONFIRMED");
    }
    Ok(availability_with(
        &[target.to_string()],
        current_direct,
        probe,
    ))
}

pub(crate) fn availability_with(
    devices: &[String],
    current_direct: Option<&str>,
    mut probe: impl FnMut(&str) -> Result<(), PreflightError>,
) -> Vec<DeviceAvailability> {
    devices
        .iter()
        .map(|id| {
            let state = match disposition(id, current_direct) {
                ProbeDisposition::Current => Availability::CurrentSonante,
                ProbeDisposition::Unresolved => Availability::Indeterminate,
                ProbeDisposition::Probe => match probe(id) {
                    Ok(()) => Availability::Available,
                    Err(PreflightError::Busy) => Availability::Busy,
                    Err(PreflightError::Unavailable | PreflightError::Unknown) => {
                        Availability::Indeterminate
                    }
                },
            };
            DeviceAvailability {
                id: id.clone(),
                state,
            }
        })
        .collect()
}

impl PreflightError {
    fn from_errno(errno: i32) -> Self {
        match errno {
            libc::EBUSY => Self::Busy,
            libc::ENODEV | libc::ENOENT | libc::ENXIO => Self::Unavailable,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Busy => "ALSA_PREFLIGHT_OUTPUT_BUSY",
            Self::Unavailable => "ALSA_PREFLIGHT_OUTPUT_UNAVAILABLE",
            Self::Unknown => "ALSA_PREFLIGHT_FAILED",
        }
    }
}

pub(crate) fn probe(device: &str) -> Result<(), PreflightError> {
    probe_with(device, |name, direction, nonblock| {
        PCM::open(name, direction, nonblock).map_err(|error| error.errno())
    })
}

fn probe_with<H>(
    device: &str,
    open: impl FnOnce(&CStr, Direction, bool) -> Result<H, i32>,
) -> Result<(), PreflightError> {
    // Avoid PCM::new(), whose CString conversion panics on an embedded NUL.
    let name = CString::new(device).map_err(|_| PreflightError::Unknown)?;
    let handle = open(&name, Direction::Playback, true).map_err(PreflightError::from_errno)?;
    // PCM's Drop calls snd_pcm_close. No parameters, prepare, start or samples.
    // Release before any subsequent transaction step, including MPD restart.
    drop(handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn availability_classifies_safe_probe_results_and_skips_virtual_pcm() {
        let devices = [
            "hw:CARD=Free,DEV=0",
            "hw:CARD=Busy,DEV=0",
            "hw:CARD=Missing,DEV=0",
            "hw:CARD=Unknown,DEV=0",
            "default",
            "null",
            "plughw:0,0",
            "hw:",
            "hw:SECRET\0",
        ]
        .map(str::to_string);
        let mut probed = Vec::new();
        let results = availability_with(&devices, None, |id| {
            probed.push(id.to_string());
            if id.contains("Free") {
                Ok(())
            } else if id.contains("Busy") {
                Err(PreflightError::Busy)
            } else if id.contains("Missing") {
                Err(PreflightError::Unavailable)
            } else {
                Err(PreflightError::Unknown)
            }
        });
        assert_eq!(probed, devices[..4]);
        assert_eq!(
            results.iter().map(|r| r.state).collect::<Vec<_>>(),
            [
                Availability::Available,
                Availability::Busy,
                Availability::Indeterminate,
                Availability::Indeterminate,
                Availability::Indeterminate,
                Availability::Indeterminate,
                Availability::Indeterminate,
                Availability::Indeterminate,
                Availability::Indeterminate
            ]
        );
        let json = serde_json::to_string(&results[..4]).unwrap();
        assert!(!json.contains("ALSA_PREFLIGHT"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn availability_never_probes_current_direct_or_its_recognized_alias() {
        let devices =
            ["hw:CARD=Owned,DEV=0", "hw:Owned,0", "hw:CARD=Other,DEV=0"].map(str::to_string);
        let mut probed = Vec::new();
        let results = availability_with(&devices, Some("hw:CARD=Owned,DEV=0"), |id| {
            probed.push(id.to_string());
            Ok(())
        });
        assert_eq!(probed, [devices[2].clone()]);
        assert_eq!(results[0].state, Availability::CurrentSonante);
        assert_eq!(results[1].state, Availability::CurrentSonante);
        assert_eq!(results[2].state, Availability::Available);
    }

    #[test]
    fn availability_does_not_probe_when_current_hardware_identity_is_ambiguous() {
        let devices = ["hw:CARD=Owned,DEV=0", "hw:CARD=Other,DEV=0"].map(str::to_string);
        for current in ["hw:0,0", "custom", "plughw:0,0"] {
            let results = availability_with(&devices, Some(current), |_| {
                panic!("ambiguous owner must not be probed")
            });
            assert!(results
                .iter()
                .all(|r| r.state == Availability::Indeterminate));
        }
    }

    #[test]
    fn availability_normalizes_numeric_aliases_and_rejects_unresolved_owners() {
        for (target, current, expected) in [
            ("hw:00,00", "hw:0", Availability::CurrentSonante),
            (
                "hw:CARD=0,DEV=000",
                "hw:000,0",
                Availability::CurrentSonante,
            ),
            (
                "hw:CARD=Owned,DEV=00",
                "hw:Owned",
                Availability::CurrentSonante,
            ),
            ("hw:0,0", "hw:CARD=Owned,DEV=0", Availability::Indeterminate),
            ("hw:CARD=Owned,DEV=0", "hw:0,0", Availability::Indeterminate),
            ("hw:CARD=Other,DEV=0", "custom", Availability::Indeterminate),
            (
                "hw:CARD=Other,DEV=0",
                "hw:9999999999999,0",
                Availability::Indeterminate,
            ),
        ] {
            assert!(!should_probe(target, Some(current)));
            let result = availability_with(&[target.to_string()], Some(current), |_| {
                panic!("owned or ambiguous aliases must never be opened")
            });
            assert_eq!(result[0].state, expected);
        }
    }

    #[test]
    fn directed_availability_validates_enumeration_before_opening_only_the_target() {
        let devices = ["hw:CARD=Free,DEV=0", "hw:CARD=Other,DEV=0"].map(str::to_string);
        let mut opened = Vec::new();
        let result = availability_for_with(&devices[0], &devices, None, |id| {
            opened.push(id.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(opened, [devices[0].clone()]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, Availability::Available);
        for target in [
            "hw:CARD=Unknown,DEV=0",
            "default",
            "plughw:0,0",
            "hw:Free,0",
            "hw:SECRET\0",
        ] {
            assert_eq!(
                availability_for_with(target, &devices, None, |_| {
                    panic!("unlisted or nonphysical names must not open ALSA")
                })
                .unwrap_err(),
                "ALSA_AVAILABILITY_UNCONFIRMED"
            );
        }
        let owned = availability_for_with(
            "hw:CARD=Absent,DEV=0",
            &devices,
            Some("hw:CARD=Absent,DEV=0"),
            |_| panic!("absent saved owner must not be probed"),
        )
        .unwrap();
        assert_eq!(owned[0].state, Availability::CurrentSonante);
    }

    #[test]
    fn directed_availability_preserves_safe_error_states_without_raw_text() {
        let target = "hw:CARD=Test,DEV=0";
        for (error, expected) in [
            (PreflightError::Busy, Availability::Busy),
            (PreflightError::Unavailable, Availability::Indeterminate),
            (PreflightError::Unknown, Availability::Indeterminate),
        ] {
            let result =
                availability_for_with(target, &[target.to_string()], None, |_| Err(error)).unwrap();
            assert_eq!(result[0].state, expected);
            assert!(!serde_json::to_string(&result)
                .unwrap()
                .contains("ALSA_PREFLIGHT"));
        }
    }

    struct Handle<'a>(&'a Cell<usize>);
    impl Drop for Handle<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn alsa_preflight_opens_only_nonblocking_playback_and_closes_before_return() {
        let closed = Cell::new(0);
        probe_with("hw:CARD=Test,DEV=0", |name, direction, nonblock| {
            assert_eq!(name.to_bytes(), b"hw:CARD=Test,DEV=0");
            assert_eq!(direction, Direction::Playback);
            assert!(nonblock);
            assert_eq!(closed.get(), 0);
            Ok(Handle(&closed))
        })
        .unwrap();
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn alsa_preflight_classifies_errno_without_raw_text() {
        for (errno, expected) in [
            (libc::EBUSY, PreflightError::Busy),
            (libc::ENODEV, PreflightError::Unavailable),
            (libc::ENOENT, PreflightError::Unavailable),
            (libc::ENXIO, PreflightError::Unavailable),
            (libc::EACCES, PreflightError::Unknown),
            (libc::EIO, PreflightError::Unknown),
            (libc::EINVAL, PreflightError::Unknown),
        ] {
            let error = probe_with::<()>("hw:0,0", |_, _, _| Err(errno)).unwrap_err();
            assert_eq!(error, expected);
            assert!(error.code().starts_with("ALSA_PREFLIGHT_"));
            assert!(!error.code().contains("hw:"));
        }
    }

    #[test]
    fn alsa_preflight_invalid_name_does_not_open_or_expose_data() {
        let error = probe_with::<()>("hw:SECRET\0https://user:password.invalid", |_, _, _| {
            panic!("invalid C string must not reach ALSA")
        })
        .unwrap_err();
        assert_eq!(error.code(), "ALSA_PREFLIGHT_FAILED");
    }
}
