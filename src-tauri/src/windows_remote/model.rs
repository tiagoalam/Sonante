//! Original LAN schema with opaque bindings to the real Windows queue.
use crate::remote::{self, Command, QueuePage, QueueTrack, Snapshot};
use crate::windows_mpd::{action::Action, library::Track};
use serde_json::json;
fn clean(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}
pub(crate) struct Frame {
    pub generation: u64,
    pub snapshot: Snapshot,
    pub revision: String,
    pub count: usize,
}
pub(crate) fn key(track: Option<&Track>, index: usize) -> String {
    let media=track.and_then(|t|{
  if let Some(u)=&t.upnp{return Some(json!({"kind":"upnp","session_id":u.session_id,"queue_index":index}));}
  if let Some(r)=&t.radio{return Some(json!({"kind":"radio","station_id":r.station_id,"queue_index":index}));}
  match &t.remote_source{
   Some(crate::windows_mpd::queue_v3::Source::Plex{server_id,part_key,..})=>Some(json!({"kind":"plex","server_id":server_id,"part_key":part_key,"queue_index":index})),
   Some(crate::windows_mpd::queue_v3::Source::Navidrome{song_id,..})=>t.remote_connection_id.as_ref().map(|c|json!({"kind":"navidrome","connection_id":c,"song_id":song_id,"queue_index":index})),
   None|Some(crate::windows_mpd::queue_v3::Source::Local{..})=>Some(json!({"kind":"local","uri":crate::windows_mpd::library::track_uri(t),"queue_index":index})),
  }
 });
    remote::digest(&serde_json::to_vec(&media).unwrap_or_default())
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn frame(
    generation: u64,
    running: bool,
    state: &str,
    logical: &str,
    elapsed: f64,
    duration: f64,
    volume: u8,
    audio: &str,
    queue: &[Track],
    index: usize,
    revision: &str,
    consistent: bool,
    warning: bool,
    direct: bool,
    muted: bool,
) -> Frame {
    let track = queue.get(index);
    let source = track
        .map(|t| {
            if t.upnp.is_some(){"UPnP"}else if t.radio.is_some() {
                "Radio"
            } else {
                match t.remote_source {
                    Some(crate::windows_mpd::queue_v3::Source::Plex { .. }) => "Plex",
                    Some(crate::windows_mpd::queue_v3::Source::Navidrome { .. }) => "Navidrome",
                    None | Some(crate::windows_mpd::queue_v3::Source::Local { .. }) => "Local",
                }
            }
        })
        .unwrap_or("");
    let state = if !running && track.is_some() {
        logical
    } else {
        state
    };
    Frame {
        generation,
        revision: format!("q1:{}", remote::digest(revision.as_bytes())),
        count: queue.len(),
        snapshot: Snapshot {
            schema: 1,
            available: consistent && !warning,
            state: match state {
                "play" => "play",
                "pause" => "pause",
                _ => "stop",
            }
            .into(),
            elapsed: if elapsed.is_finite() {
                elapsed.max(0.)
            } else {
                0.
            },
            duration: if duration.is_finite() {
                duration.max(0.)
            } else {
                0.
            },
            media: key(track, index),
            title: clean(track.map_or("", |t| &t.title), 512),
            artist: clean(track.map_or("", |t| &t.artist), 512),
            album: clean(track.map_or("", |t| &t.album), 512),
            source: source.into(),
            audio_format: clean(audio, 100),
            index: track.map(|_| index),
            volume: volume.min(100) as u32,
            muted,
            volume_writable: track.is_some() && !direct && consistent && !warning,
        },
    }
}
pub(crate) fn queue(
    frame: &Frame,
    tracks: &[Track],
    durations: &[Option<f64>],
    offset: usize,
) -> Result<QueuePage, String> {
    if offset > tracks.len() {
        return Err("REMOTE_CHANGED".into());
    }
    let known = durations.len() == tracks.len();
    Ok(QueuePage {
        revision: frame.revision.clone(),
        total: tracks.len(),
        offset,
        tracks: tracks
            .iter()
            .enumerate()
            .skip(offset)
            .take(60)
            .map(|(index, t)| QueueTrack {
                index,
                title: clean(&t.title, 512),
                artist: clean(&t.artist, 512),
                album: clean(&t.album, 512),
                duration: known
                    .then(|| durations[index])
                    .flatten()
                    .filter(|v| v.is_finite() && *v > 0.),
                artwork: None,
            })
            .collect(),
    })
}
pub(crate) fn resolve(command: &Command, frame: &Frame) -> Result<Action, &'static str> {
    if !command.valid() || !frame.snapshot.available {
        return Err("REMOTE_CHANGED");
    }
    let wanted = match command {
        Command::Play { media }
        | Command::Pause { media }
        | Command::Next { media }
        | Command::Previous { media }
        | Command::Seek { media, .. } => Some(media),
        _ => None,
    };
    if wanted.is_some_and(|v| v != &frame.snapshot.media)
        || wanted.is_some() && frame.snapshot.index.is_none()
    {
        return Err("REMOTE_CHANGED");
    }
    Ok(match command {
        Command::Play { .. } => Action::SetPlaying { want_play: true },
        Command::Pause { .. } => Action::SetPlaying { want_play: false },
        Command::Next { .. } => Action::Next,
        Command::Previous { .. } => Action::Previous,
        Command::Seek { seconds, .. } => Action::Seek {
            seconds: *seconds,
            index: frame.snapshot.index,
        },
        Command::Volume { value } => {
            if !frame.snapshot.volume_writable {
                return Err("DIRECT_VOLUME_POLICY");
            }
            Action::Volume {
                value: *value as u8,
            }
        }
        Command::QueuePlay { index, revision } => {
            if revision != &frame.revision || *index >= frame.count {
                return Err("QUEUE_EDIT_CHANGED");
            }
            Action::Jump { index: *index }
        }
    })
}
