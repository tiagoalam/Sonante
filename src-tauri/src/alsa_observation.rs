//! A bounded, read-only sample of the selected ALSA substream, not a bit test.
//! Never opens a PCM device, sends MPD commands, or reads audio samples.
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    Unavailable,
    NotPlaying,
    Shared,
    Unreadable,
    Malformed,
    NotOwned,
    Ambiguous,
    Changed,
    Limit,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Observation {
    Unverified {
        reason: Reason,
    },
    Observed {
        configured_endpoint: String,
        card: u32,
        device: u32,
        subdevice: u32,
        format: String,
        rate: u32,
        channels: u32,
        usb_device: Option<UsbDeviceIdentity>,
        kernel_release: Option<String>,
    },
}
// Generic, bounded USB descriptors. Profiles are evaluated separately; no
// serial number, firmware claim or certification of the physical unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct UsbDeviceIdentity {
    vendor_id: String,
    product_id: String,
    model: String,
}
#[derive(Debug, PartialEq, Eq)]
struct UsbIdentity {
    path: PathBuf,
    vendor: String,
    product: String,
    model: String,
}
impl UsbIdentity {
    fn evidence(&self) -> UsbDeviceIdentity {
        UsbDeviceIdentity {
            vendor_id: self.vendor.clone(),
            product_id: self.product.clone(),
            model: self.model.clone(),
        }
    }
}
fn usb_identity(
    sys: &Path, card: u32,
    read: &mut impl FnMut(&Path) -> Result<String, Reason>,
) -> Option<UsbIdentity> {
    // Follow only the selected card's device link. No USB/card enumeration,
    // serial numbers, PCM opens or USB transfers. Reads share the procfs budget.
    let interface = sys.join(format!("class/sound/card{card}/device")).canonicalize().ok()?;
    if !interface.starts_with(sys.join("devices")) { return None; }
    let driver = fs::read_link(interface.join("driver")).ok()?;
    if driver.file_name()?.to_str()? != "snd-usb-audio" { return None; }
    let path = interface.parent()?.to_path_buf();
    let vendor = read(&path.join("idVendor")).ok()?.trim().to_ascii_lowercase();
    let product = read(&path.join("idProduct")).ok()?.trim().to_ascii_lowercase();
    let model = read(&path.join("product")).ok()?.trim().to_owned();
    if ![&vendor, &product].iter().all(|v| v.len() == 4 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        || model.is_empty() || model.len() > 128 || model.chars().any(char::is_control) { return None; }
    Some(UsbIdentity { path: interface, vendor, product, model })
}

fn kernel_release(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 128
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c)))
        .then(|| value.to_owned())
}

impl Observation {
    pub(crate) fn unverified(reason: Reason) -> Self {
        Self::Unverified { reason }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Context {
    pub pid: u32,
    pub endpoint: String,
    pub shared: bool,
}

static READING: AtomicBool = AtomicBool::new(false);
struct Permit<'a>(&'a AtomicBool);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn enter(flag: &AtomicBool) -> Option<Permit<'_>> {
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| Permit(flag))
}

fn number(value: &str, maximum: u32) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok().filter(|n| *n <= maximum)
}
// Only a physical ALSA address, never a path or arbitrary PCM configuration.
fn address(value: &str) -> Option<(&str, u32)> {
    let mut parts = value.strip_prefix("hw:")?.split(',');
    let card = parts.next()?;
    let card = card.strip_prefix("CARD=").unwrap_or(card);
    if card.is_empty()
        || card.len() > 32
        || !card
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return None;
    }
    let device = parts.next()?;
    let device = number(device.strip_prefix("DEV=").unwrap_or(device), 255)?;
    if parts.next().is_some() {
        return None;
    }
    Some((card, device))
}
fn field<'a>(text: &'a str, key: &str) -> Result<&'a str, Reason> {
    let mut found = text.lines().filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then_some(value.trim())
    });
    let value = found.next().ok_or(Reason::Malformed)?;
    if value.is_empty() || found.next().is_some() {
        return Err(Reason::Malformed);
    }
    Ok(value)
}
fn owner(text: &str) -> Result<u32, Reason> {
    if field(text, "state")? != "RUNNING" {
        return Err(Reason::NotPlaying);
    }
    number(field(text, "owner_pid")?, u32::MAX)
        .filter(|n| *n > 0)
        .ok_or(Reason::Malformed)
}
fn process_start(text: &str, pid: u32) -> Result<u64, Reason> {
    let (prefix, _) = text.split_once(' ').ok_or(Reason::Malformed)?;
    if number(prefix, u32::MAX) != Some(pid) {
        return Err(Reason::Changed);
    }
    // comm may itself contain spaces and ')'. Field 22 is starttime.
    let tail = text.rsplit_once(") ").ok_or(Reason::Malformed)?.1;
    let fields: Vec<_> = tail.split_whitespace().collect();
    if matches!(fields.first(), Some(&"Z" | &"X")) {
        return Err(Reason::Unavailable);
    }
    fields
        .get(19)
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or(Reason::Malformed)
}
fn thread_belongs(text: &str, tid: u32, pid: u32) -> Result<(), Reason> {
    if number(field(text, "Pid")?, u32::MAX) != Some(tid)
        || number(field(text, "Tgid")?, u32::MAX) != Some(pid)
    {
        return Err(Reason::NotOwned);
    }
    Ok(())
}
#[derive(Debug, PartialEq, Eq)]
struct Parameters {
    format: String,
    rate: u32,
    channels: u32,
}
fn parameters(text: &str) -> Result<Parameters, Reason> {
    let format = field(text, "format")?;
    // No raw kernel strings cross IPC. DSD labels describe ALSA containers only.
    if ![
        "S8",
        "U8",
        "S16_LE",
        "S16_BE",
        "S24_LE",
        "S24_BE",
        "S24_3LE",
        "S24_3BE",
        "S32_LE",
        "S32_BE",
        "FLOAT_LE",
        "FLOAT_BE",
        "FLOAT64_LE",
        "FLOAT64_BE",
        "DSD_U8",
        "DSD_U16_LE",
        "DSD_U16_BE",
        "DSD_U32_LE",
        "DSD_U32_BE",
    ]
    .contains(&format)
    {
        return Err(Reason::Malformed);
    }
    let access = field(text, "access")?;
    if ![
        "RW_INTERLEAVED",
        "RW_NONINTERLEAVED",
        "MMAP_INTERLEAVED",
        "MMAP_NONINTERLEAVED",
        "MMAP_COMPLEX",
    ]
    .contains(&access)
    {
        return Err(Reason::Malformed);
    }
    if field(text, "subformat")? != "STD" {
        return Err(Reason::Malformed);
    }
    let rate_text = field(text, "rate")?;
    let mut parts = rate_text.split_whitespace();
    let rate = number(parts.next().ok_or(Reason::Malformed)?, 50_000_000)
        .filter(|r| *r >= 1_000)
        .ok_or(Reason::Malformed)?;
    let rational = parts.next().ok_or(Reason::Malformed)?;
    let rational = rational
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or(Reason::Malformed)?;
    let (n, d) = rational.split_once('/').ok_or(Reason::Malformed)?;
    let n = number(n, u32::MAX).ok_or(Reason::Malformed)?;
    let d = number(d, u32::MAX)
        .filter(|v| *v > 0)
        .ok_or(Reason::Malformed)?;
    if parts.next().is_some() || u64::from(rate) * u64::from(d) != u64::from(n) {
        return Err(Reason::Malformed);
    }
    let channels = number(field(text, "channels")?, 64)
        .filter(|c| *c > 0)
        .ok_or(Reason::Malformed)?;
    Ok(Parameters {
        format: format.to_owned(),
        rate,
        channels,
    })
}

// Limits are cooperative between local procfs operations, not syscall deadlines.
struct Budget {
    began: Instant,
    bytes: usize,
    reads: usize,
}
impl Budget {
    fn new() -> Self {
        Self {
            began: Instant::now(),
            bytes: 0,
            reads: 0,
        }
    }
    fn check(&self) -> Result<(), Reason> {
        if self.began.elapsed() > Duration::from_millis(50)
            || self.reads >= 100
            || self.bytes >= 128 * 1024
        {
            Err(Reason::Limit)
        } else {
            Ok(())
        }
    }
    fn read(&mut self, path: &Path) -> Result<String, Reason> {
        self.check()?;
        self.reads += 1;
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|_| Reason::Unreadable)?
            .take(8193)
            .read_to_end(&mut bytes)
            .map_err(|_| Reason::Unreadable)?;
        self.bytes += bytes.len();
        if bytes.len() > 8192
            || self.bytes > 128 * 1024
            || self.began.elapsed() > Duration::from_millis(50)
        {
            return Err(Reason::Limit);
        }
        String::from_utf8(bytes).map_err(|_| Reason::Malformed)
    }
}

pub(crate) fn observe(context: &Context) -> Observation {
    if context.shared {
        return Observation::unverified(Reason::Shared);
    }
    let Some(_permit) = enter(&READING) else {
        return Observation::unverified(Reason::Busy);
    };
    let mut budget = Budget::new();
    let result = observe_in(Path::new("/proc"), Path::new("/sys"), context, &mut |path| budget.read(path));
    result.unwrap_or_else(Observation::unverified)
}

// Sample only a stable confirmed context. The caller releases all application
// locks before read() and invalidates during Save, shutdown or process changes.
pub(crate) fn sample(
    playing: bool,
    before: Option<Context>,
    mut current: impl FnMut() -> Option<Context>,
    read: impl FnOnce(&Context) -> Observation,
) -> Observation {
    if !playing {
        return Observation::unverified(Reason::NotPlaying);
    }
    let Some(before) = before else {
        return Observation::unverified(Reason::Unavailable);
    };
    if current().as_ref() != Some(&before) {
        return Observation::unverified(Reason::Changed);
    }
    if before.shared {
        return Observation::unverified(Reason::Shared);
    }
    let result = read(&before);
    if current().as_ref() != Some(&before) {
        return Observation::unverified(Reason::Changed);
    }
    result
}

fn subdevices_in(directory: &Path) -> Result<Vec<u32>, Reason> {
    let mut subdevices = Vec::new();
    // Bounded enumeration of just the selected playback device, never all cards.
    for (index, entry) in fs::read_dir(directory)
        .map_err(|_| Reason::Unreadable)?
        .enumerate()
    {
        if index >= 64 {
            return Err(Reason::Limit);
        }
        let entry = entry.map_err(|_| Reason::Unreadable)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(Reason::Malformed)?;
        if let Some(subdevice) = name.strip_prefix("sub") {
            subdevices.push(number(subdevice, 31).ok_or(Reason::Limit)?);
            if subdevices.len() > 16 {
                return Err(Reason::Limit);
            }
        }
    }
    subdevices.sort_unstable();
    Ok(subdevices)
}

fn observe_in(
    proc: &Path,
    sys: &Path,
    context: &Context,
    read: &mut impl FnMut(&Path) -> Result<String, Reason>,
) -> Result<Observation, Reason> {
    let (card_id, device) = address(&context.endpoint).ok_or(Reason::Unavailable)?;
    let asound = proc.join("asound");
    let card = if card_id.bytes().all(|b| b.is_ascii_digit()) {
        number(card_id, 1023).ok_or(Reason::Malformed)?
    } else {
        let alias = fs::read_link(asound.join(card_id)).map_err(|_| Reason::Unreadable)?;
        let alias = alias.to_str().ok_or(Reason::Malformed)?;
        let card = alias
            .strip_prefix("card")
            .and_then(|v| number(v, 1023))
            .ok_or(Reason::Malformed)?;
        if read(&asound.join(format!("card{card}/id")))?.trim() != card_id {
            return Err(Reason::Changed);
        }
        card
    };
    // The release is immutable for this boot. One bounded local read uses the
    // existing observation budget, with no shell command or separate timer.
    let kernel = read(&proc.join("sys/kernel/osrelease")).ok().and_then(|s| kernel_release(&s));
    let usb_before = usb_identity(sys, card, read);
    let process = proc.join(context.pid.to_string());
    let start = process_start(&read(&process.join("stat"))?, context.pid)?;
    let directory = asound.join(format!("card{card}/pcm{device}p"));
    let subdevices = subdevices_in(&directory)?;
    let initial_subdevices = subdevices.clone();
    let mut observed = None;
    let mut unavailable = false;
    for subdevice in subdevices {
        let path = directory.join(format!("sub{subdevice}"));
        let status = match read(&path.join("status")) {
            Ok(s) => s,
            Err(_) => {
                unavailable = true;
                continue;
            }
        };
        if status.trim() == "closed" {
            continue;
        }
        let tid = match owner(&status) {
            Ok(tid) => tid,
            Err(Reason::NotPlaying) => continue,
            Err(_) => {
                unavailable = true;
                continue;
            }
        };
        // ALSA owner_pid can be the MPD output thread's TID, not the main PID.
        let task = process.join(format!("task/{tid}/status"));
        let task_before = match read(&task) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if thread_belongs(&task_before, tid, context.pid).is_err() {
            continue;
        }
        let hw = parameters(&read(&path.join("hw_params"))?)?;
        let status_after = read(&path.join("status"))?;
        if owner(&status_after)? != tid
            || parameters(&read(&path.join("hw_params"))?)? != hw
            || thread_belongs(&read(&task)?, tid, context.pid).is_err()
        {
            return Err(Reason::Changed);
        }
        if observed.is_some() {
            return Err(Reason::Ambiguous);
        }
        observed = Some(Observation::Observed {
            configured_endpoint: context.endpoint.clone(),
            card,
            device,
            subdevice,
            format: hw.format,
            rate: hw.rate,
            channels: hw.channels,
            usb_device: usb_before.as_ref().map(UsbIdentity::evidence),
            kernel_release: kernel.clone(),
        });
    }
    if process_start(&read(&process.join("stat"))?, context.pid)? != start {
        return Err(Reason::Changed);
    }
    if subdevices_in(&directory)? != initial_subdevices {
        return Err(Reason::Changed);
    }
    if !card_id.bytes().all(|b| b.is_ascii_digit())
        && (fs::read_link(asound.join(card_id)).map_err(|_| Reason::Changed)?
            != Path::new(&format!("card{card}"))
            || read(&asound.join(format!("card{card}/id")))?.trim() != card_id)
    {
        return Err(Reason::Changed);
    }

    if usb_identity(sys, card, read) != usb_before {
        return Err(Reason::Changed);
    }
    // A readable match cannot hide an unreadable/ambiguous second substream.
    if unavailable {
        return Err(Reason::Unreadable);
    }
    observed.ok_or(Reason::NotOwned)
}

#[cfg(test)]
mod tests;
