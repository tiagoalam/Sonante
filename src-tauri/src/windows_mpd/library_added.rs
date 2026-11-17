//! Catalog admission events, not filesystem timestamps. Legacy entries remain unknown.
use super::{id, Result};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, fs::OpenOptions, io::Read, path::{Path, PathBuf}};
#[path = "../windows_navidrome/unique_json.rs"]
mod unique;
const ERROR: &str = "library_added_unavailable";
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 150_000;
const MAX_ROOTS: usize = 4096;
const MAX_TIME: u64 = 253402300799;
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    roots: BTreeSet<String>,
    tracks: BTreeMap<String, Option<u64>>,
}
pub(super) fn path(config: &Path) -> PathBuf { config.with_extension("added-v1.json") }
fn hash_valid(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
fn load(path: &Path) -> Result<Saved> {
    crate::persistence::prepare_private_file_for_load(path, ERROR).map_err(|_| ERROR)?;
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Saved { version: 1, ..Saved::default() }),
        Err(_) => return Err(ERROR),
    };
    let mut bytes = Vec::new(); file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| ERROR)?;
    if bytes.len() as u64 > MAX_BYTES { return Err(ERROR); }
    let saved: Saved = unique::readonly_unique_json(&bytes).map_err(|_| ERROR)?;
    if saved.version != 1 || saved.roots.len() > MAX_ROOTS || saved.tracks.len() > MAX_ENTRIES
        || saved.roots.iter().any(|s| !hash_valid(s))
        || saved.tracks.iter().any(|(k,t)| !hash_valid(k) || t.is_some_and(|t| t > MAX_TIME)) { return Err(ERROR); }
    Ok(saved)
}
/// Called only after enumeration completes, under Library.change. A partial scan
/// reads known dates but never writes or initializes a legacy root baseline.
pub(super) fn observe(config: &Path, legacy_roots: &BTreeSet<String>, roots: &[PathBuf],
    rows: &[(String, String)], complete: bool, now: u64, current: impl Fn() -> bool,
) -> Result<BTreeMap<String, Option<u64>>> {
    if !current() { return Err("library_changed"); }
    let path = path(config);
    let _lock = crate::persistence::operation_lock(&path.with_extension("lock")).map_err(|_| ERROR)?;
    let mut saved = load(&path)?;
    if complete {
        if now > MAX_TIME { return Err(ERROR); }
        let mut changed = !path.exists();
        for (track, root) in rows {
            if !hash_valid(track) || !hash_valid(root) { return Err(ERROR); }
            if !saved.tracks.contains_key(track) {
                // Restored folders have no recoverable Added history on upgrade.
                let date = if legacy_roots.contains(root) && !saved.roots.contains(root) { None } else { Some(now) };
                saved.tracks.insert(track.clone(), date); changed = true;
            }
        }
        for root in roots { changed |= saved.roots.insert(id(&root.to_string_lossy())); }
        if saved.tracks.len() > MAX_ENTRIES || saved.roots.len() > MAX_ROOTS { return Err(ERROR); }
        if !current() { return Err("library_changed"); }
        if changed {
            let bytes = serde_json::to_vec(&saved).map_err(|_| ERROR)?;
            crate::persistence::atomic_write_private(&path, &bytes, ERROR).map_err(|_| ERROR)?;
        }
    }
    Ok(saved.tracks)
}
/// Gregorian UTC, identical wire format to Linux MPD Added. No local timezone.
pub(super) fn utc(seconds: u64) -> Option<String> {
    if seconds > MAX_TIME { return None; }
    let z = (seconds / 86400) as i64 + 719468;
    let era = z / 146097; let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400; let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 }; y += i64::from(m <= 2);
    Some(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60))
}

#[cfg(test)]
#[path = "library_added_tests.rs"]
mod tests;
