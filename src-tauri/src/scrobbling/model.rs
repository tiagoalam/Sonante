//! Pure listening accounting. Only explicit Local/Plex metadata reaches a service.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Lastfm,
    Listenbrainz,
}
impl Provider {
    pub fn index(self) -> usize {
        match self {
            Self::Lastfm => 0,
            Self::Listenbrainz => 1,
        }
    }
    pub fn all() -> [Self; 2] {
        [Self::Lastfm, Self::Listenbrainz]
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: u32,
}
fn text(value: &str, required: bool) -> Option<String> {
    let value = value.trim();
    if (required && value.is_empty())
        || value.len() > 1024
        || value.chars().count() > 256
        || value.chars().any(char::is_control)
        || value.contains("://")
        || value.starts_with(['/', '\\'])
        || value.to_ascii_lowercase().contains("x-plex-token=")
    {
        return None;
    }
    Some(value.to_string())
}
impl Track {
    pub fn new(artist: &str, title: &str, album: &str, duration: f64) -> Option<Self> {
        if !duration.is_finite() || duration <= 30. || duration > 86_400. {
            return None;
        }
        let artist = text(artist, true)?;
        if [
            "unknown artist",
            "artista desconhecido",
            "unknown",
            "desconhecido",
        ]
        .contains(&artist.to_lowercase().as_str())
        {
            return None;
        }
        Some(Self {
            artist,
            title: text(title, true)?,
            album: text(album, false)?,
            duration: duration.ceil() as u32,
        })
    }
    pub fn valid(&self) -> bool {
        Self::new(
            &self.artist,
            &self.title,
            &self.album,
            f64::from(self.duration),
        )
        .is_some()
    }
    fn threshold(&self) -> f64 {
        (f64::from(self.duration) / 2.).min(240.)
    }
}
pub struct Observation {
    pub at: Instant,
    pub utc: u64,
    pub key: Option<String>,
    pub playing: bool,
    pub stopped: bool,
    pub gap: bool,
    pub elapsed: f64,
    pub track: Option<Track>,
}
#[derive(Clone)]
pub struct Target {
    pub account: String,
    pub since: Instant,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub id: u64,
    pub provider: Provider,
    pub account: String,
    pub track: Track,
    pub started_at: u64,
    pub attempts: u8,
    pub next_attempt: u64,
}
#[derive(Clone)]
pub struct NowPlaying {
    pub provider: Provider,
    pub account: String,
    pub track: Track,
    pub observed: Instant,
}
#[derive(Default)]
struct Progress {
    account: String,
    listened: f64,
    now_playing: bool,
    submitted: bool,
    was_enabled: bool,
}
struct Current {
    key: String,
    track: Track,
    started_at: u64,
    last_at: Instant,
    last_elapsed: f64,
    playing: bool,
    progress: [Progress; 2],
}
#[derive(Default)]
pub struct Tracker {
    current: Option<Current>,
    last_observation: Option<Instant>,
}
impl Tracker {
    pub fn suspend(&mut self, provider: Provider) {
        if let Some(c) = &mut self.current {
            let p = &mut c.progress[provider.index()];
            p.was_enabled = false;
            if !p.submitted {
                p.listened = 0.;
            }
        }
    }
    pub fn observe(
        &mut self,
        o: Observation,
        targets: &[Option<Target>; 2],
    ) -> (Vec<NowPlaying>, Vec<Pending>) {
        let mut np = vec![];
        let mut listens = vec![];
        if self.last_observation.is_some_and(|at| o.at <= at) {
            return (np, listens);
        }
        self.last_observation = Some(o.at);
        if o.gap {
            if let Some(c) = &mut self.current {
                c.playing = false;
                c.last_at = o.at;
            }
            return (np, listens);
        }
        if o.stopped {
            self.current = None;
            return (np, listens);
        }
        let (Some(key), Some(track)) = (o.key, o.track) else {
            self.current = None;
            return (np, listens);
        };
        if !o.elapsed.is_finite() || o.elapsed < 0. {
            return (np, listens);
        }
        let repeat = self.current.as_ref().is_some_and(|c| {
            c.key == key
                && c.playing
                && o.playing
                && c.last_elapsed >= f64::from(c.track.duration) - 2.
                && o.elapsed <= 3.
                && c.last_elapsed - o.elapsed > f64::from(c.track.duration) / 2.
                && o.at.saturating_duration_since(c.last_at) <= Duration::from_secs(5)
        });
        if repeat || self.current.as_ref().is_none_or(|c| c.key != key) {
            self.current = Some(Current {
                key,
                track,
                started_at: o.utc,
                last_at: o.at,
                last_elapsed: o.elapsed,
                playing: false,
                progress: std::array::from_fn(|_| Progress::default()),
            });
        }
        let c = self.current.as_mut().unwrap();
        let wall = o.at.saturating_duration_since(c.last_at).as_secs_f64();
        let delta = o.elapsed - c.last_elapsed;
        // Large gaps, stalled output and seek jumps give no listening credit.
        let credit = if c.playing && o.playing && wall <= 5. && delta >= 0. && delta <= wall + 1.5 {
            delta.min(wall)
        } else {
            0.
        };
        for provider in Provider::all() {
            let p = &mut c.progress[provider.index()];
            let Some(target) = &targets[provider.index()] else {
                p.was_enabled = false;
                if !p.submitted {
                    p.listened = 0.;
                }
                continue;
            };
            if o.at < target.since {
                p.was_enabled = false;
                continue;
            }
            if p.account != target.account {
                *p = Progress {
                    account: target.account.clone(),
                    ..Progress::default()
                };
            }
            if o.playing {
                if !p.now_playing {
                    p.now_playing = true;
                    np.push(NowPlaying {
                        provider,
                        account: p.account.clone(),
                        track: c.track.clone(),
                        observed: o.at,
                    });
                }
                if p.was_enabled && c.last_at >= target.since {
                    p.listened += credit;
                }
                if !p.submitted && p.listened + 0.001 >= c.track.threshold() {
                    p.submitted = true;
                    listens.push(Pending {
                        id: 0,
                        provider,
                        account: p.account.clone(),
                        track: c.track.clone(),
                        started_at: c.started_at,
                        attempts: 0,
                        next_attempt: o.utc,
                    });
                }
            }
            p.was_enabled = true;
        }
        c.last_at = o.at;
        c.last_elapsed = o.elapsed;
        c.playing = o.playing;
        (np, listens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(duration: f64) -> Track {
        Track::new("Synthetic Artist", "Synthetic Song", "Album", duration).unwrap()
    }
    fn targets(at: Instant) -> [Option<Target>; 2] {
        std::array::from_fn(|_| {
            Some(Target {
                account: "fixture".into(),
                since: at,
            })
        })
    }
    fn sample(at: Instant, elapsed: f64, playing: bool, duration: f64) -> Observation {
        Observation {
            at,
            utc: 1_700_000_000 + elapsed.max(0.) as u64,
            key: Some("local:occurrence:1".into()),
            playing,
            stopped: false,
            gap: false,
            elapsed,
            track: Some(track(duration)),
        }
    }
    #[test]
    fn real_listening_qualifies_once_per_provider_and_not_at_start() {
        for (duration, threshold) in [(60., 30), (600., 240)] {
            let at = Instant::now();
            let targets = targets(at);
            let mut t = Tracker::default();
            let (np, p) = t.observe(sample(at, 0., true, duration), &targets);
            assert_eq!(np.len(), 2);
            assert!(p.is_empty());
            for n in 1..threshold {
                assert!(t
                    .observe(
                        sample(at + Duration::from_secs(n), n as f64, true, duration),
                        &targets
                    )
                    .1
                    .is_empty());
            }
            let (np, p) = t.observe(
                sample(
                    at + Duration::from_secs(threshold),
                    threshold as f64,
                    true,
                    duration,
                ),
                &targets,
            );
            assert!(np.is_empty());
            assert_eq!(p.len(), 2);
            assert!(t
                .observe(
                    sample(
                        at + Duration::from_secs(threshold + 1),
                        (threshold + 1) as f64,
                        true,
                        duration
                    ),
                    &targets
                )
                .1
                .is_empty());
        }
    }
    #[test]
    fn pauses_seeks_stalls_and_unobserved_gaps_cannot_scrobble() {
        let at = Instant::now();
        let targets = targets(at);
        let mut t = Tracker::default();
        t.observe(sample(at, 0., true, 100.), &targets);
        for (wall, elapsed, play) in [
            (1, 80., true),
            (2, 80., false),
            (100, 80., false),
            (101, 80., true),
            (102, 0., true),
            (200, 95., true),
            (201, 95., true),
        ] {
            assert!(t
                .observe(
                    sample(at + Duration::from_secs(wall), elapsed, play, 100.),
                    &targets
                )
                .1
                .is_empty());
        }
    }
    #[test]
    fn stopped_replays_and_real_end_repeat_are_new_occurrences() {
        let at = Instant::now();
        let targets = targets(at);
        let mut t = Tracker::default();
        t.observe(sample(at, 0., true, 32.), &targets);
        for n in 1..=31 {
            t.observe(
                sample(at + Duration::from_secs(n), n as f64, true, 32.),
                &targets,
            );
        }
        let (np, _) = t.observe(
            sample(at + Duration::from_secs(32), 0., true, 32.),
            &targets,
        );
        assert_eq!(np.len(), 2);
        let mut stop = sample(at + Duration::from_secs(33), 0., false, 32.);
        stop.stopped = true;
        t.observe(stop, &targets);
        assert_eq!(
            t.observe(
                sample(at + Duration::from_secs(34), 0., true, 32.),
                &targets
            )
            .0
            .len(),
            2
        );
    }
    #[test]
    fn no_retroactive_opt_in_cross_account_or_reenable_duplicate() {
        let at = Instant::now();
        let targets = targets(at);
        let mut t = Tracker::default();
        t.observe(sample(at, 0., true, 60.), &targets);
        for n in 1..=30 {
            t.observe(
                sample(at + Duration::from_secs(n), n as f64, true, 60.),
                &targets,
            );
        }
        t.observe(
            sample(at + Duration::from_secs(31), 31., true, 60.),
            &[None, None],
        );
        assert!(t
            .observe(
                sample(at + Duration::from_secs(32), 32., true, 60.),
                &targets
            )
            .1
            .is_empty());
        let later = targets.map(|v| {
            v.map(|mut v| {
                v.since = at + Duration::from_secs(40);
                v.account = "different".into();
                v
            })
        });
        assert!(t
            .observe(sample(at + Duration::from_secs(33), 33., true, 60.), &later)
            .0
            .is_empty());
        assert!(t
            .observe(sample(at + Duration::from_secs(41), 41., true, 60.), &later)
            .1
            .is_empty());
    }
    #[test]
    fn uncertain_snapshot_does_not_create_another_occurrence_or_credit_the_gap() {
        let at = Instant::now();
        let targets = targets(at);
        let mut tracker = Tracker::default();
        tracker.observe(sample(at, 0., true, 60.), &targets);
        for n in 1..=30 {
            tracker.observe(
                sample(at + Duration::from_secs(n), n as f64, true, 60.),
                &targets,
            );
        }
        let mut gap = sample(at + Duration::from_secs(31), 31., true, 60.);
        gap.gap = true;
        gap.key = None;
        gap.track = None;
        tracker.observe(gap, &targets);
        let (np, pending) = tracker.observe(
            sample(at + Duration::from_secs(32), 32., true, 60.),
            &targets,
        );
        assert!(np.is_empty());
        assert!(pending.is_empty());
    }
    #[test]
    fn metadata_rejects_missing_unknown_credentials_paths_and_short_tracks() {
        for artist in [
            "",
            "Unknown Artist",
            "/private/artist",
            "https://host",
            "bad\nvalue",
        ] {
            assert!(Track::new(artist, "Song", "Album", 120.).is_none());
        }
        for title in [
            "",
            "/private/file.flac",
            "https://host/?X-Plex-Token=secret",
            "bad\nname",
        ] {
            assert!(Track::new("Artist", title, "", 120.).is_none());
        }
        for d in [0., 30., f64::NAN, f64::INFINITY, 100_000.] {
            assert!(Track::new("Artist", "Song", "Album", d).is_none());
        }
        assert!(Track::new("AC/DC", "Song", "", 120.).is_some());
    }
}
