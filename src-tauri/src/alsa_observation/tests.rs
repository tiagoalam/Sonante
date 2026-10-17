use super::*;
use std::{cell::Cell, path::PathBuf, sync::atomic::AtomicU64};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    context: Context,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn stat(pid: u32, start: u64) -> String {
    let mut fields = vec!["0".to_string(); 20];
    fields[0] = "S".into();
    fields[19] = start.to_string();
    format!("{pid} (mpd output) thread) {}", fields.join(" "))
}
fn hw(format: &str, rate: u32, channels: u32) -> String {
    format!("access: RW_INTERLEAVED\nformat: {format}\nsubformat: STD\nchannels: {channels}\nrate: {rate} ({rate}/1)\nperiod_size: 4096\nbuffer_size: 16384\n")
}
fn status(tid: u32) -> String {
    format!("state: RUNNING\nowner_pid : {tid}\ntrigger_time: 1.2\ndelay: 12\n")
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "sonante-alsa-observation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let f = Self {
            root,
            context: Context {
                pid: 110,
                endpoint: "hw:2,0".into(),
                shared: false,
            },
        };
        f.write("110/stat", &stat(110, 987));
        f.write("110/task/115/status", "Name:\tmpd\nPid:\t115\nTgid:\t110\n");
        f.write("asound/card2/id", "FixtureDAC\n");
        f.sub(0, 115);
        f
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn sub(&self, sub: u32, tid: u32) {
        self.write(&format!("asound/card2/pcm0p/sub{sub}/status"), &status(tid));
        self.write(
            &format!("asound/card2/pcm0p/sub{sub}/hw_params"),
            &hw("S32_LE", 96000, 2),
        );
    }
    fn read(&self, path: &Path) -> Result<String, Reason> {
        fs::read_to_string(path).map_err(|_| Reason::Unreadable)
    }
    fn observe(&self) -> Result<Observation, Reason> {
        observe_in(&self.root, &self.root.join("sys"), &self.context, &mut |p| self.read(p))
    }
}
#[test]
fn physical_addresses_reject_paths_plugins_and_extra_arguments() {
    for value in ["hw:2,0", "hw:CARD=FixtureDAC,DEV=0", "hw:FixtureDAC,0"] {
        assert!(address(value).is_some(), "{value}");
    }
    for value in [
        "default",
        "plughw:2,0",
        "hw:../../private,0",
        "hw:2,0,0",
        "hw:2,0?token=secret",
        "hw:2",
        "hw:2,-1",
        "hw:2,256",
        "hw:CARD=,DEV=0",
        "hw:2,0\n",
    ] {
        assert!(address(value).is_none(), "{value}");
    }
}
#[test]
fn parses_kernel_containers_without_inferring_codec_dop_or_bitperfect() {
    for (format, rate) in [
        ("S16_LE", 44100),
        ("S24_3LE", 96000),
        ("S24_LE", 192000),
        ("S32_LE", 96000),
        ("FLOAT_LE", 48000),
        ("DSD_U32_BE", 352800),
    ] {
        assert_eq!(
            parameters(&hw(format, rate, 2)),
            Ok(Parameters {
                format: format.into(),
                rate,
                channels: 2
            })
        );
    }
    for value in [
        hw("secret", 96000, 2),
        hw("S32_LE", 0, 2),
        hw("S32_LE", 96000, 0),
        hw("S32_LE", 96000, 65),
        hw("S32_LE", 96000, 2) + "format: S16_LE\n",
        hw("S32_LE", 96000, 2).replace("96000/1", "96001/1"),
        hw("S32_LE", 96000, 2).replace("96000/1", "96000/0"),
        hw("S32_LE", 96000, 2).replace("subformat: STD", "subformat: MSBITS_20"),
        "closed\n".into(),
    ] {
        assert_eq!(parameters(&value), Err(Reason::Malformed));
    }
}
#[test]
fn observes_only_the_selected_running_substream_owned_by_mpd_thread() {
    let f = Fixture::new();
    let expected = Observation::Observed {
        configured_endpoint: "hw:2,0".into(),
        card: 2,
        device: 0,
        subdevice: 0,
        format: "S32_LE".into(),
        rate: 96000,
        channels: 2,
        usb_device: None,
        kernel_release: None,
    };
    assert_eq!(f.observe(), Ok(expected));
    f.write("110/task/115/status", "Pid: 115\nTgid: 999\n");
    assert_eq!(f.observe(), Err(Reason::NotOwned));
}
#[test]
fn resolves_symbolic_card_alias_only_when_kernel_id_agrees() {
    let mut f = Fixture::new();
    std::os::unix::fs::symlink("card2", f.root.join("asound/FixtureDAC")).unwrap();
    f.context.endpoint = "hw:CARD=FixtureDAC,DEV=0".into();
    assert!(matches!(
        f.observe(),
        Ok(Observation::Observed { card: 2, .. })
    ));
    f.write("asound/card2/id", "ReplacedDAC\n");
    assert_eq!(f.observe(), Err(Reason::Changed));
    fs::remove_file(f.root.join("asound/FixtureDAC")).unwrap();
    std::os::unix::fs::symlink("../private", f.root.join("asound/FixtureDAC")).unwrap();
    assert_eq!(f.observe(), Err(Reason::Malformed));
}
#[test]
fn closed_prepared_unrelated_missing_and_duplicate_streams_never_verify() {
    let f = Fixture::new();
    f.sub(1, 999); // Unrelated identical-format stream is not evidence.
    assert!(matches!(
        f.observe(),
        Ok(Observation::Observed { subdevice: 0, .. })
    ));
    f.sub(1, 115);
    assert_eq!(f.observe(), Err(Reason::Ambiguous));
    f.write("asound/card2/pcm0p/sub1/status", "closed\n");
    assert!(f.observe().is_ok());
    fs::remove_file(f.root.join("asound/card2/pcm0p/sub1/status")).unwrap();
    assert_eq!(f.observe(), Err(Reason::Unreadable));
    fs::remove_dir_all(f.root.join("asound/card2/pcm0p/sub1")).unwrap();
    f.write(
        "asound/card2/pcm0p/sub0/status",
        "state: PREPARED\nowner_pid:115\n",
    );
    assert_eq!(f.observe(), Err(Reason::NotOwned));
    fs::remove_dir_all(f.root.join("asound/card2/pcm0p")).unwrap();
    assert_eq!(f.observe(), Err(Reason::Unreadable));
}
#[test]
fn rejects_format_owner_and_process_identity_changes_during_sample() {
    for mode in ["format", "owner", "process", "thread"] {
        let f = Fixture::new();
        let count = Cell::new(0);
        let result = observe_in(&f.root, &f.root.join("sys"), &f.context, &mut |p| {
            let suffix = if mode == "format" {
                "hw_params"
            } else if mode == "owner" {
                "sub0/status"
            } else if mode == "thread" {
                "task/115/status"
            } else {
                "110/stat"
            };
            if p.ends_with(suffix) {
                let n = count.get();
                count.set(n + 1);
                if n > 0 {
                    return Ok(match mode {
                        "format" => hw("S16_LE", 44100, 2),
                        "owner" => status(999),
                        "thread" => "Pid:115\nTgid:999\n".into(),
                        _ => stat(110, 988),
                    });
                }
            }
            f.read(p)
        });
        assert_eq!(result, Err(Reason::Changed), "{mode}");
    }
}
#[test]
fn bounded_files_enumeration_and_gate_have_explicit_limits_and_cleanup() {
    let f = Fixture::new();
    f.write("large", &"x".repeat(8193));
    let mut b = Budget::new();
    assert_eq!(b.read(&f.root.join("large")), Err(Reason::Limit));
    for n in 1..17 {
        f.sub(n, 999);
    }
    assert_eq!(f.observe(), Err(Reason::Limit));
    let flag = AtomicBool::new(false);
    let permit = enter(&flag).unwrap();
    assert!(enter(&flag).is_none());
    drop(permit);
    assert!(enter(&flag).is_some());
}
#[test]
fn snapshot_gate_skips_pause_stop_shared_save_and_discards_changed_context() {
    let context = Context {
        pid: 110,
        endpoint: "hw:2,0".into(),
        shared: false,
    };
    for playing in [false, true] {
        let result = sample(
            playing,
            None,
            || panic!("no context"),
            |_| panic!("no proc reads"),
        );
        assert_eq!(
            result,
            Observation::unverified(if playing {
                Reason::Unavailable
            } else {
                Reason::NotPlaying
            })
        );
    }
    let shared = Context {
        shared: true,
        ..context.clone()
    };
    assert_eq!(
        sample(
            true,
            Some(shared.clone()),
            || Some(shared.clone()),
            |_| panic!("Shared never scans hardware")
        ),
        Observation::unverified(Reason::Shared)
    );
    assert_eq!(
        sample(
            true,
            Some(context.clone()),
            || None,
            |_| panic!("Save/exit before read")
        ),
        Observation::unverified(Reason::Changed)
    );
    let mut checks = 0;
    assert_eq!(
        sample(
            true,
            Some(context.clone()),
            || {
                checks += 1;
                if checks == 1 {
                    Some(context.clone())
                } else {
                    None
                }
            },
            |_| Observation::unverified(Reason::NotOwned)
        ),
        Observation::unverified(Reason::Changed)
    );
}
#[test]
fn only_sanitized_observation_crosses_ipc_and_read_only_sample_preserves_fixture() {
    let f = Fixture::new();
    let before = f
        .read(&f.root.join("asound/card2/pcm0p/sub0/hw_params"))
        .unwrap();
    let json = serde_json::to_string(&f.observe().unwrap()).unwrap();
    assert!(json.contains("S32_LE"));
    for sensitive in [
        "owner_pid",
        "110",
        "115",
        "/proc",
        "fixture",
        "bitperfect",
        "native",
        "dop",
    ] {
        assert!(!json.contains(sensitive), "{json}");
    }
    assert_eq!(
        f.read(&f.root.join("asound/card2/pcm0p/sub0/hw_params"))
            .unwrap(),
        before
    );
}

#[test]
fn newly_added_substream_or_replaced_card_alias_invalidates_sample() {
    for mode in ["stream", "alias"] {
        let mut f = Fixture::new();
        if mode == "alias" {
            std::os::unix::fs::symlink("card2", f.root.join("asound/FixtureDAC")).unwrap();
            f.context.endpoint = "hw:CARD=FixtureDAC,DEV=0".into();
        }
        let count = Cell::new(0);
        let result = observe_in(&f.root, &f.root.join("sys"), &f.context, &mut |p| {
            if p.ends_with("hw_params") {
                count.set(count.get() + 1);
                if count.get() == 2 {
                    if mode == "stream" {
                        f.sub(1, 115);
                    } else {
                        fs::remove_file(f.root.join("asound/FixtureDAC")).unwrap();
                        std::os::unix::fs::symlink("card3", f.root.join("asound/FixtureDAC"))
                            .unwrap();
                    }
                }
            }
            f.read(p)
        });
        assert_eq!(result, Err(Reason::Changed));
    }
}

fn usb_fixture(f: &Fixture, card: u32, model: &str) {
    use std::os::unix::fs::symlink;
    f.write("sys/devices/usb3/3-2/idVendor", "152a\n");
    f.write("sys/devices/usb3/3-2/idProduct", "8750\n");
    f.write("sys/devices/usb3/3-2/product", model);
    let interface = f.root.join("sys/devices/usb3/3-2/3-2:1.0");
    fs::create_dir_all(&interface).unwrap();
    symlink("/sys/bus/usb/drivers/snd-usb-audio", interface.join("driver")).unwrap();
    let card = f.root.join(format!("sys/class/sound/card{card}"));
    fs::create_dir_all(&card).unwrap();
    symlink(interface, card.join("device")).unwrap();
}
#[test]
fn selected_usb_descriptors_are_generic_and_independent_of_card_number_or_serial_identity() {
    let f = Fixture::new();
    usb_fixture(&f, 2, "E70\n");
    assert!(matches!(f.observe(), Ok(Observation::Observed { usb_device: Some(ref usb), .. }) if usb.vendor_id == "152a" && usb.product_id == "8750" && usb.model == "E70"));
    let identity = usb_identity(&f.root.join("sys"), 2, &mut |p| f.read(p)).unwrap();
    // A card renumbering or a different USB port does not change the model match.
    let alias = f.root.join("sys/class/sound/card7");
    fs::create_dir_all(&alias).unwrap();
    std::os::unix::fs::symlink(&identity.path, alias.join("device")).unwrap();
    assert_eq!(usb_identity(&f.root.join("sys"), 7, &mut |p| f.read(p)).unwrap().evidence(), identity.evidence());
    let mut reads = Vec::new();
    assert!(usb_identity(&f.root.join("sys"), 2, &mut |p| { reads.push(p.to_owned()); f.read(p) }).is_some());
    assert_eq!(reads.len(), 3);
    assert!(!reads.iter().any(|p| p.ends_with("serial")));
    assert!(!serde_json::to_string(&f.observe().unwrap()).unwrap().contains("/sys/"));
}
#[test]
fn other_usb_models_are_identified_but_missing_or_invalid_descriptors_remain_unknown() {
    let f = Fixture::new();
    assert!(matches!(f.observe(), Ok(Observation::Observed { usb_device: None, .. })));
    usb_fixture(&f, 2, "Other");
    assert!(matches!(f.observe(), Ok(Observation::Observed { usb_device: Some(ref usb), .. }) if usb.model == "Other"));
    f.write("sys/devices/usb3/3-2/product", "E70");
    f.write("sys/devices/usb3/3-2/idProduct", "0001");
    assert!(matches!(f.observe(), Ok(Observation::Observed { usb_device: Some(ref usb), .. }) if usb.model == "E70" && usb.product_id == "0001"));
    f.write("sys/devices/usb3/3-2/idProduct", "invalid");
    assert!(matches!(f.observe(), Ok(Observation::Observed { usb_device: None, .. })));
    fs::remove_file(f.root.join("sys/devices/usb3/3-2/idVendor")).unwrap();
    assert!(matches!(f.observe(), Ok(Observation::Observed { usb_device: None, .. })));
}
#[test]
fn changing_selected_usb_identity_during_proc_observation_invalidates_the_sample() {
    let f = Fixture::new();
    usb_fixture(&f, 2, "E70");
    let mut changed = false;
    let result = observe_in(&f.root, &f.root.join("sys"), &f.context, &mut |p| {
        if !changed && p.ends_with("110/stat") {
            f.write("sys/devices/usb3/3-2/product", "Other");
            changed = true;
        }
        f.read(p)
    });
    assert_eq!(result, Err(Reason::Changed));
}

#[test]
fn kernel_release_is_bounded_and_only_reported_with_owned_current_observation() {
    for value in ["6.12.108-1-MANJARO", "7.1.0-custom_1+test"] {
        assert_eq!(kernel_release(&format!("{value}\n")), Some(value.into()));
    }
    for value in ["", "version with spaces", "6.12\nsecret", "../kernel/path", &"x".repeat(129)] {
        assert_eq!(kernel_release(value), None);
    }
    let f = Fixture::new();
    f.write("sys/kernel/osrelease", "6.12.108-1-MANJARO\n");
    let mut reads = 0;
    let result = observe_in(&f.root, &f.root.join("sys"), &f.context, &mut |p| {
        if p.ends_with("sys/kernel/osrelease") { reads += 1; }
        f.read(p)
    });
    assert!(matches!(result, Ok(Observation::Observed { kernel_release: Some(ref version), .. }) if version == "6.12.108-1-MANJARO"));
    assert_eq!(reads, 1);
    f.write("sys/kernel/osrelease", "invalid version");
    assert!(matches!(f.observe(), Ok(Observation::Observed { kernel_release: None, .. })));
    f.write("sys/kernel/osrelease", "6.12.108-1-MANJARO");
    f.write("110/task/115/status", "Pid: 115\nTgid: 999\n");
    assert_eq!(f.observe(), Err(Reason::NotOwned));
}
