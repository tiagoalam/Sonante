//! Owned-session software unity/mute only. Never writes an ALSA hardware mixer.
use crate::audio::{PlaybackStatus, VolumeBackend};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const ACTIVE: u64 = 1;
const PREPARED: u64 = 2;
const MUTED: u64 = 4;
const CHANGED: u64 = 8;
const BUSY: u64 = 16;
const STEP: u64 = 32;
pub(crate) const CONFIRM: &str = "DIRECT_VOLUME_CONFIRMATION_REQUIRED";
pub(crate) const STALE: &str = "DIRECT_VOLUME_CONTEXT_CHANGED";
pub(crate) const GRADUAL: &str = "DIRECT_VOLUME_ON_EQUIPMENT";
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Gain {
    Pending,
    Unity,
    Muted,
    Changed,
}
#[derive(Debug)]
pub(crate) struct Session {
    state: AtomicU64,
    id: u64,
}
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) backend: VolumeBackend,
    session: Option<Arc<Session>>,
    stamp: u64,
}
impl Context {
    pub(crate) fn new(backend: VolumeBackend, session: Option<Arc<Session>>) -> Self {
        let stamp = session
            .as_ref()
            .map_or(0, |s| s.state.load(Ordering::Acquire));
        Self {
            backend,
            session,
            stamp,
        }
    }
    pub(crate) fn validate_token(&self, token: &str) -> Result<(), String> {
        if self.backend != VolumeBackend::DirectEquipment
            || self.stamp & (ACTIVE | BUSY) != ACTIVE
            || !self.session.as_ref().is_some_and(|session| {
                session.state.load(Ordering::Acquire) == self.stamp && session.token() == token
            })
        {
            return Err(STALE.into());
        }
        Ok(())
    }
    pub(crate) fn apply(&self, after: &Self, playback: &mut PlaybackStatus) -> bool {
        if self.backend != VolumeBackend::DirectEquipment || after.backend != self.backend {
            return false;
        }
        playback.volume.backend = VolumeBackend::DirectEquipment;
        playback.volume.writable = false;
        playback.volume.direct_token = None;
        let valid = self
            .session
            .as_ref()
            .zip(after.session.as_ref())
            .filter(|(a, b)| Arc::ptr_eq(a, b) && self.stamp == after.stamp);
        let gain = valid.and_then(|(s, _)| {
            s.inspect(
                self.stamp,
                playback
                    .processing
                    .as_ref()
                    .and_then(|p| p.mpd_volume)
                    .map(u32::from),
            )
        });
        if let Some(gain) = gain {
            playback.volume.direct_token = self.session.as_ref().map(|s| s.token());
            playback.volume.direct_gain = Some(gain);
            playback.volume.muted = gain == Gain::Muted && playback.volume.value == 0;
        } else {
            playback.volume.direct_gain = Some(Gain::Changed);
            playback.volume.available = false;
            playback.volume.muted = false;
            playback.snapshot_consistent = false;
        }
        true
    }
}
#[derive(Debug, Clone, PartialEq)]
struct Status {
    volume: u32,
    state: String,
    song: Option<String>,
    playlist: Option<String>,
}
fn status(lines: Vec<String>) -> Result<Status, String> {
    let mut volume = None;
    let mut state = None;
    let mut song = None;
    let mut playlist = None;
    for line in lines {
        if let Some((key, value)) = line.split_once(": ") {
            match key {
                "volume" => {
                    if volume.is_some() {
                        return Err(STALE.into());
                    }
                    volume = Some(
                        value
                            .parse::<u32>()
                            .ok()
                            .filter(|v| *v <= 100)
                            .ok_or(STALE)?,
                    );
                }
                "state" => {
                    if state.is_some() || !matches!(value, "play" | "pause" | "stop") {
                        return Err(STALE.into());
                    }
                    state = Some(value.to_owned());
                }
                "songid" => song = Some(value.to_owned()),
                "playlist" => playlist = Some(value.to_owned()),
                _ => {}
            }
        }
    }
    Ok(Status {
        volume: volume.ok_or(STALE)?,
        state: state.ok_or(STALE)?,
        song,
        playlist,
    })
}
// Shared values come only from an observed owned Shared software session.
// Restoration is stopped and checked; a failed read never silently becomes 100.
pub(crate) fn observed_software_volume(lines: Vec<String>) -> Result<u32, String> {
    Ok(status(lines)?.volume)
}
pub(crate) fn restore_shared_software(
    value: u32,
    mut command: impl FnMut(&str) -> Result<Vec<String>, String>,
) -> Result<(), String> {
    if value > 100 {
        return Err(STALE.into());
    }
    let before = status(command("status")?)?;
    if before.state != "stop" {
        return Err(STALE.into());
    }
    command(&format!("setvol {value}"))?;
    let after = status(command("status")?)?;
    if after.volume != value
        || after.state != "stop"
        || after.song != before.song
        || after.playlist != before.playlist
    {
        return Err(STALE.into());
    }
    Ok(())
}

impl Session {
    pub(crate) fn initialize(
        mut command: impl FnMut(&str) -> Result<Vec<String>, String>,
    ) -> Result<Arc<Self>, String> {
        let before = status(command("status")?)?;
        if before.state != "stop" {
            return Err(STALE.into());
        }
        command("setvol 0")?;
        let after = status(command("status")?)?;
        if after.volume != 0
            || after.state != "stop"
            || after.song != before.song
            || after.playlist != before.playlist
        {
            return Err(STALE.into());
        }
        Ok(Arc::new(Self {
            state: AtomicU64::new(ACTIVE),
            id: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
        }))
    }
    pub(crate) fn token(&self) -> String {
        format!("{}:{}", self.id, self.state.load(Ordering::Acquire))
    }
    pub(crate) fn invalidate(&self) {
        self.state.fetch_and(!ACTIVE, Ordering::AcqRel);
    }
    pub(crate) fn inspect(&self, stamp: u64, volume: Option<u32>) -> Option<Gain> {
        if stamp & ACTIVE == 0 || stamp & BUSY != 0 || self.state.load(Ordering::Acquire) != stamp {
            return None;
        }
        if stamp & PREPARED != 0 {
            let expected = if stamp & MUTED != 0 { 0 } else { 100 };
            if volume == Some(expected) {
                return Some(if expected == 0 {
                    Gain::Muted
                } else {
                    Gain::Unity
                });
            }
            let next = (stamp + STEP) & !(PREPARED | MUTED) | CHANGED;
            let _ = self
                .state
                .compare_exchange(stamp, next, Ordering::AcqRel, Ordering::Acquire);
            Some(Gain::Changed)
        } else if stamp & MUTED != 0 && volume == Some(0) {
            Some(Gain::Muted)
        } else {
            if volume != Some(0) {
                let _ = self.state.compare_exchange(stamp, (stamp + STEP) | CHANGED, Ordering::AcqRel, Ordering::Acquire);
                return Some(Gain::Changed);
            }
            Some(if stamp & CHANGED != 0 {
                Gain::Changed
            } else {
                Gain::Pending
            })
        }
    }
    // Called only for a current explicit playback request after route choice.
    // Initial safety zero is distinct from an intentional mute. Never fix
    // external changes or raise gain while Playing.
    pub(crate) fn before_explicit_play(&self, allow_initial_gain:bool, mut command: impl FnMut(&str)->Result<Vec<String>,String>) -> Result<(),String> {
        let stamp=self.state.load(Ordering::Acquire);
        if stamp & ACTIVE == 0 || stamp & (BUSY|CHANGED) != 0 {return Err(STALE.into());}
        let before=status(command("status")?)?;
        let gain=self.inspect(stamp,Some(before.volume)).ok_or(STALE)?;
        match gain {
            Gain::Unity => Ok(()),
            Gain::Muted => {
                // Choice was checked by the caller and zero was observed. This
                // permits a future explicit unmute; it does not report unity.
                self.state.compare_exchange(stamp,stamp|PREPARED,Ordering::AcqRel,Ordering::Acquire).map_err(|_|STALE)?;
                Ok(())
            },
            Gain::Pending if allow_initial_gain && before.volume==0 && before.state!="play" => {
                self.set_mute(false,true,&self.token(),command)
            }
            _ => Err(STALE.into()),
        }
    }
    pub(crate) fn check_active(&self) -> Result<(),String> {
        let stamp=self.state.load(Ordering::Acquire);
        if stamp & ACTIVE==0 || stamp & (BUSY|CHANGED)!=0 {Err(STALE.into())}else{Ok(())}
    }
    pub(crate) fn set_mute(
        &self,
        muted: bool,
        prepare: bool,
        token: &str,
        mut command: impl FnMut(&str) -> Result<Vec<String>, String>,
    ) -> Result<(), String> {
        let stamp = self.state.load(Ordering::Acquire);
        if token != format!("{}:{}", self.id, stamp) {
            return Err(STALE.into());
        }
        if stamp & ACTIVE == 0 || stamp & BUSY != 0 {
            return Err(STALE.into());
        }
        let busy = (stamp + STEP) | BUSY;
        self.state
            .compare_exchange(stamp, busy, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| STALE)?;
        let current = || {
            if self.state.load(Ordering::Acquire) == busy {
                Ok(())
            } else {
                Err(STALE.to_string())
            }
        };
        let result = (|| {
            let before = status(command("status")?)?;
            current()?;
            let coherent =
                stamp & PREPARED != 0 && before.volume == if stamp & MUTED != 0 { 0 } else { 100 };
            if muted {
                if before.volume != 0 {
                    command("setvol 0")?;
                }
                current()?;
                let after = status(command("status")?)?;
                if after.volume != 0 {
                    return Err(STALE.into());
                }
                return Ok(ACTIVE | MUTED | if coherent { PREPARED } else { 0 });
            }
            if !coherent {
                if !prepare {
                    return Err(CONFIRM.into());
                }
                // Confirmation prepares unit gain but never resumes playback. Re-pause in
                // the fixed command list before changing gain, even after a concurrent play.
                let hold = if before.state == "stop" {
                    "stop"
                } else {
                    "pause 1"
                };
                command(hold)?;
                current()?;
                let paused = status(command("status")?)?;
                let expected = if before.state == "stop" {
                    "stop"
                } else {
                    "pause"
                };
                if paused.state != expected
                    || paused.song != before.song
                    || paused.playlist != before.playlist
                {
                    return Err(STALE.into());
                }
                current()?;
                let after = status(command(&format!(
                    "command_list_begin\n{hold}\nsetvol 100\nstatus\ncommand_list_end"
                ))?)?;
                if after.volume != 100
                    || after.state != expected
                    || after.song != paused.song
                    || after.playlist != paused.playlist
                {
                    return Err(STALE.into());
                }
            } else if before.volume == 0 {
                command("setvol 100")?;
                current()?;
                let after = status(command("status")?)?;
                if after.volume != 100
                    || after.state != before.state
                    || after.song != before.song
                    || after.playlist != before.playlist
                {
                    return Err(STALE.into());
                }
            }
            current()?;
            Ok(ACTIVE | PREPARED)
        })();
        let flags = match &result {
            Ok(flags) => *flags,
            Err(_) => ACTIVE | CHANGED,
        };
        self.state
            .compare_exchange(
                busy,
                ((busy + STEP) & !31) | flags,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| STALE)?;
        result.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mpd {
        volume: u32,
        state: &'static str,
        song: String,
        playlist: u32,
        commands: Vec<String>,
        fail: Option<usize>,
    }
    impl Mpd {
        fn new() -> Self {
            Self {
                volume: 100,
                state: "stop",
                song: "7".into(),
                playlist: 2,
                commands: vec![],
                fail: None,
            }
        }
        fn command(&mut self, cmd: &str) -> Result<Vec<String>, String> {
            self.commands.push(cmd.into());
            if self.fail == Some(self.commands.len()) {
                return Err("ACK/timeout fixture".into());
            }
            for line in cmd.lines() {
                match line {
                    "setvol 0" => self.volume = 0,
                    "setvol 100" => self.volume = 100,
                    "pause 1" => self.state = "pause",
                    "stop" => self.state = "stop",
                    "status" | "command_list_begin" | "command_list_end" => {}
                    _ => panic!("unexpected command: {line}"),
                }
            }
            if cmd.contains("status") {
                Ok(vec![
                    format!("volume: {}", self.volume),
                    format!("state: {}", self.state),
                    format!("songid: {}", self.song),
                    format!("playlist: {}", self.playlist),
                ])
            } else {
                Ok(vec![])
            }
        }
        fn init(&mut self) -> Arc<Session> {
            Session::initialize(|c| self.command(c)).unwrap()
        }
        fn mute(&mut self, s: &Session, muted: bool, prepare: bool) -> Result<(), String> {
            s.set_mute(muted, prepare, &s.token(), |c| self.command(c))
        }
    }
    #[test]
    fn startup_is_stopped_zero_pending_never_unity_or_hardware() {
        let mut m = Mpd::new();
        let s = m.init();
        assert_eq!(m.volume, 0);
        assert_eq!(m.state, "stop");
        assert_eq!(
            s.inspect(s.state.load(Ordering::Acquire), Some(0)),
            Some(Gain::Pending)
        );
        assert_eq!(m.commands, ["status", "setvol 0", "status"]);
    }
    #[test]
    fn startup_refuses_playing_missing_duplicate_and_invalid_volume() {
        let mut m = Mpd::new();
        m.state = "play";
        assert!(Session::initialize(|c| m.command(c)).is_err());
        assert_eq!(m.volume, 100);
        for lines in [
            vec!["state: stop"],
            vec!["state: stop", "volume: -1"],
            vec!["state: stop", "volume: 101"],
            vec!["state: stop", "volume: 0", "volume: 0"],
        ] {
            assert!(status(lines.into_iter().map(str::to_owned).collect()).is_err());
        }
    }
    #[test]
    fn no_confirmation_never_increases_or_plays() {
        let mut m = Mpd::new();
        let s = m.init();
        m.state = "play";
        assert_eq!(m.mute(&s, false, false), Err(CONFIRM.into()));
        assert_eq!(m.volume, 0);
        assert_eq!(m.state, "play");
        assert!(!m.commands.iter().any(|c| c.contains("setvol 100")));
    }
    #[test]
    fn confirmation_pauses_or_stops_without_queue_or_position_commands() {
        for state in ["play", "pause", "stop"] {
            let mut m = Mpd::new();
            let s = m.init();
            m.state = state;
            let queue = (m.song.clone(), m.playlist);
            m.mute(&s, false, true).unwrap();
            assert_eq!(m.volume, 100);
            assert_eq!(m.state, if state == "stop" { "stop" } else { "pause" });
            assert_eq!((m.song, m.playlist), queue);
            assert_eq!(
                s.inspect(s.state.load(Ordering::Acquire), Some(100)),
                Some(Gain::Unity)
            );
            assert!(!m
                .commands
                .iter()
                .any(|c| c.lines().any(|l| l.starts_with("play")
                    || l.starts_with("seek")
                    || l.starts_with("add")
                    || l == "clear")));
        }
    }
    #[test]
    fn explicit_mute_unmute_preserves_playing_paused_stopped_and_never_resumes() {
        for state in ["play", "pause", "stop"] {
            let mut m = Mpd::new();
            let s = m.init();
            m.mute(&s, false, true).unwrap();
            m.state = state;
            m.mute(&s, true, false).unwrap();
            assert_eq!(m.volume, 0);
            assert_eq!(
                s.inspect(s.state.load(Ordering::Acquire), Some(0)),
                Some(Gain::Muted)
            );
            m.mute(&s, false, false).unwrap();
            assert_eq!(m.volume, 100);
            assert_eq!(m.state, state);
        }
    }
    #[test]
    fn external_gain_divergence_requires_new_confirmation_even_for_dsd() {
        for volume in [0, 1, 43, 99] {
            let mut m = Mpd::new();
            let s = m.init();
            m.mute(&s, false, true).unwrap();
            m.volume = volume;
            assert_eq!(
                s.inspect(s.state.load(Ordering::Acquire), Some(volume)),
                Some(Gain::Changed)
            );
            let before = m.volume;
            assert_eq!(m.mute(&s, false, false), Err(CONFIRM.into()));
            assert_eq!(m.volume, before);
        }
    }
    #[test]
    fn mute_unknown_attenuation_does_not_authorize_unmute() {
        let mut m = Mpd::new();
        let s = m.init();
        m.volume = 43;
        m.mute(&s, true, false).unwrap();
        assert_eq!(m.volume, 0);
        assert_eq!(m.mute(&s, false, false), Err(CONFIRM.into()));
        assert_eq!(m.volume, 0);
    }
    #[test]
    fn stale_confirmation_and_shutdown_have_no_commands() {
        let mut m = Mpd::new();
        let s = m.init();
        let token = s.token();
        m.mute(&s, true, false).unwrap();
        let count = m.commands.len();
        assert_eq!(
            s.set_mute(false, true, &token, |c| m.command(c)),
            Err(STALE.into())
        );
        assert_eq!(m.commands.len(), count);
        s.invalidate();
        assert_eq!(m.mute(&s, false, true), Err(STALE.into()));
        assert_eq!(m.commands.len(), count);
    }
    #[test]
    fn all_preparation_failures_remain_unprepared_and_never_play() {
        for fail in 1..=4 {
            let mut m = Mpd::new();
            let s = m.init();
            m.state = "play";
            m.fail = Some(m.commands.len() + fail);
            assert!(m.mute(&s, false, true).is_err());
            assert_eq!(
                s.inspect(s.state.load(Ordering::Acquire), Some(m.volume)),
                Some(Gain::Changed)
            );
            assert!(!m
                .commands
                .iter()
                .any(|c| c.lines().any(|l| l == "play" || l == "pause 0")));
        }
    }
    #[test]
    fn concurrent_command_and_invalidation_reject_late_commit() {
        let mut m = Mpd::new();
        let s = m.init();
        let token = s.token();
        let mut first = true;
        assert_eq!(
            s.set_mute(false, true, &token, |c| {
                if first {
                    first = false;
                    s.invalidate();
                }
                m.command(c)
            }),
            Err(STALE.into())
        );
        assert_eq!(m.volume, 0);
    }
    #[test]
    fn shared_restore_uses_only_its_own_value_and_never_plays() {
        for value in [0, 43, 100] {
            let mut m = Mpd::new();
            restore_shared_software(value, |c| {
                m.commands.push(c.into());
                if let Some(v) = c.strip_prefix("setvol ") {
                    m.volume = v.parse().unwrap();
                    Ok(vec![])
                } else {
                    m.command(c)
                }
            })
            .unwrap();
            assert_eq!(m.volume, value);
            assert_eq!(m.state, "stop");
        }
        let mut m = Mpd::new();
        m.state = "play";
        assert!(restore_shared_software(43, |c| m.command(c)).is_err());
        assert_eq!(m.volume, 100);
    }
    #[test]
    fn ambiguous_gain_write_does_not_authorize_automatic_unmute() {
        let mut m = Mpd::new();
        let s = m.init();
        m.state = "play";
        assert!(s
            .set_mute(false, true, &s.token(), |c| {
                let result = m.command(c);
                if c.contains("setvol 100") {
                    Err("lost response".into())
                } else {
                    result
                }
            })
            .is_err());
        assert_eq!(m.state, "pause");
        assert_eq!(m.volume, 100);
        assert_eq!(m.mute(&s, false, false), Err(CONFIRM.into()));
    }
    #[test]
    fn queue_changes_before_preparation_refuse_gain_increase() {
        let mut m = Mpd::new();
        let s = m.init();
        m.state = "play";
        assert!(s
            .set_mute(false, true, &s.token(), |c| {
                if c == "pause 1" {
                    m.playlist += 1;
                }
                m.command(c)
            })
            .is_err());
        assert_eq!(m.volume, 0);
        assert_eq!(m.state, "pause");
    }
}

#[cfg(test)]
#[path = "direct_volume_real_tests.rs"]
mod real_tests;
