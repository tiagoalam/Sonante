//! Read-only ICY projection. Stream URLs remain backend-only and never become paths.
use super::{
    library::Track,
    protocol::{self, Playback, Result},
};
pub(super) fn project(
    track: &mut Track,
    state: &mut Playback,
    lines: Option<&[String]>,
    expected_id: u64,
) -> Result<()> {
    let station = track.radio.as_ref().ok_or("RADIO_INVALID")?;
    let mut title = station.station_name.clone();
    let mut album = String::new();
    if state.state == "play" {
        let lines = lines.ok_or("mpd_protocol")?;
        let id = protocol::field(lines, "Id")?
            .map(|id| id.parse::<u64>().map_err(|_| "mpd_protocol"))
            .transpose()?;
        if id.is_some_and(|id| id != expected_id)
            || protocol::field(lines, "Pos")?.is_some_and(|pos| pos != "0")
        {
            return Err("mpd_protocol");
        }
        // A stream can end between status and currentsong. No song ID means
        // station fallback, not a false engine failure or unbound ICY metadata.
        if id.is_some() {
            // MPD can send repeated multi-value tags. Match the original Local/
            // radio parser's last-value policy, while IDs remain single-valued.
            let (mut tag_title, mut tag_artist, mut tag_album) = ("", "", "");
            for line in lines {
                if let Some((key, value)) = line.split_once(": ") {
                    match key {
                        "Title" => tag_title = value,
                        "Artist" => tag_artist = value,
                        "Album" => tag_album = value,
                        _ => {}
                    }
                }
            }
            if !tag_title.trim().is_empty() {
                title = tag_title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(300)
                    .collect();
            }
            album = if !tag_artist.trim().is_empty() {
                tag_artist
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect()
            } else {
                tag_album.into()
            };
        }
    }
    // Reset every observation; a stopped/tagless stream cannot retain the
    // preceding song. These presentation tags never change the station ID/URL.
    track.title = title;
    track.artist = station.station_name.clone();
    track.album = album;
    state.duration = 0.;
    Ok(())
}
pub(super) fn require_station(track: Option<&Track>, id: &str) -> Result<()> {
    if !track
        .and_then(|t| t.radio.as_ref())
        .is_some_and(|radio| radio.station_id == id)
    {
        return Err("RADIO_STATION_CHANGED");
    }
    Ok(())
}
pub(super) fn stop_action(track:Option<&Track>,id:&str,eq:bool)->Result<Option<super::action::Action>>{
    require_station(track,id)?;
    // Reuse the existing actor's fresh-generation Stop. Sending native Stop
    // directly would invalidate PCM and turn a successful radio stop into an
    // EQ_CONTINUITY_LOST error on the following observation.
    Ok(eq.then_some(super::action::Action::Stop))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn station() -> Track {
        crate::windows_radio::Station {
            id: format!("manual:{}", "a".repeat(64)),
            name: "Synthetic Station".into(),
            country_code: String::new(),
            tags: vec![],
            codec: String::new(),
            bitrate: 0,
            favicon: Some("https://fixture.invalid/logo.jpg".into()),
            stream_url: "https://stream.fixture.invalid/live?key=PRIVATE".into(),
            manual: true,
        }
        .selection()
        .unwrap()
        .tracks
        .remove(0)
    }
    #[test]
    fn station_scoped_eq_stop_uses_generation_action_and_rejects_stale_station_before_effects(){
        let track=station();let id=track.radio.as_ref().unwrap().station_id.as_str();
        assert!(matches!(stop_action(Some(&track),id,true),Ok(Some(super::super::action::Action::Stop))));
        assert!(matches!(stop_action(Some(&track),id,false),Ok(None)));
        for eq in [false,true]{
            assert!(matches!(stop_action(Some(&track),"different-station",eq),Err("RADIO_STATION_CHANGED")));
            assert!(matches!(stop_action(None,id,eq),Err("RADIO_STATION_CHANGED")));
        }
    }
    fn state(mode: &str) -> Playback {
        Playback {
            state: mode.into(),
            elapsed: 12.,
            duration: 999.,
            volume: 40,
            audio: "44100:16:2".into(),
        }
    }
    fn tags(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn original_icy_titles_station_artist_and_tag_artist_album_policy_is_preserved() {
        let mut track = station();
        let mut value = state("play");
        let before = track.radio.as_ref().unwrap().stream_url.clone();
        project(
            &mut track,
            &mut value,
            Some(&tags(&[
                "Id: 19",
                "Pos: 0",
                "Title: Old",
                "Title: Current Song",
                "Artist: Current Artist",
            ])),
            19,
        )
        .unwrap();
        assert_eq!(
            (
                track.title.as_str(),
                track.artist.as_str(),
                track.album.as_str()
            ),
            ("Current Song", "Synthetic Station", "Current Artist")
        );
        assert_eq!(value.duration, 0.);
        assert_eq!(track.radio.as_ref().unwrap().stream_url, before);
        project(
            &mut track,
            &mut value,
            Some(&tags(&["Id: 19", "Album: Broadcast Album"])),
            19,
        )
        .unwrap();
        assert_eq!(track.title, "Synthetic Station");
        assert_eq!(track.album, "Broadcast Album");
        project(&mut track, &mut value, Some(&tags(&["Id: 19"])), 19).unwrap();
        assert_eq!(track.album, "");
        for mode in ["stop", "pause"] {
            let mut value = state(mode);
            project(&mut track, &mut value, None, 19).unwrap();
            assert_eq!(track.title, "Synthetic Station");
            assert_eq!(track.album, "");
            assert_eq!(value.duration, 0.);
        }
    }
    #[test]
    fn icy_bounds_control_characters_and_missing_song_after_eof() {
        let mut track = station();
        let mut value = state("play");
        let lines = vec![
            "Id: 19".into(),
            format!("Title: {}\u{1}", "音".repeat(500)),
            format!("Artist: {}\u{7}", "A".repeat(500)),
        ];
        project(&mut track, &mut value, Some(&lines), 19).unwrap();
        assert_eq!(track.title.chars().count(), 300);
        assert_eq!(track.album.len(), 200);
        project(&mut track, &mut value, Some(&[]), 19).unwrap();
        assert_eq!(track.title, "Synthetic Station");
        assert_eq!(track.album, "");
    }
    #[test]
    fn contradictory_identity_fails_without_mutating_presentation() {
        for rows in [
            vec!["Id: 20"],
            vec!["Id: 19", "Id: 19"],
            vec!["Id: bad"],
            vec!["Id: 19", "Pos: 1"],
        ] {
            let mut track = station();
            let mut value = state("play");
            let before = serde_json::to_value(&track).unwrap();
            assert_eq!(
                project(&mut track, &mut value, Some(&tags(&rows)), 19),
                Err("mpd_protocol")
            );
            assert_eq!(serde_json::to_value(&track).unwrap(), before);
            assert_eq!(value.duration, 999.);
        }
    }
    #[test]
    fn public_track_omits_stream_url_and_native_local_fields_are_not_paths_to_url() {
        let track = station();
        let json = serde_json::to_string(&track).unwrap();
        assert!(
            !json.contains("PRIVATE")
                && !json.contains("stream_url")
                && !json.contains("stationName")
        );
        assert!(track.path.as_os_str().is_empty());
        assert!(track.media_root.is_none());
        assert!(track.catalog_album_id.is_none());
        assert_eq!(
            track.radio.as_ref().unwrap().station_name,
            "Synthetic Station"
        );
    }
    #[test]
    fn stop_requires_the_current_radio_identity_and_cannot_stop_a_local_source() {
        let mut track = station();
        let id = track.radio.as_ref().unwrap().station_id.clone();
        assert_eq!(require_station(Some(&track), &id), Ok(()));
        assert_eq!(
            require_station(Some(&track), "other"),
            Err("RADIO_STATION_CHANGED")
        );
        track.radio = None;
        assert_eq!(
            require_station(Some(&track), &id),
            Err("RADIO_STATION_CHANGED")
        );
        assert_eq!(require_station(None, &id), Err("RADIO_STATION_CHANGED"));
    }
}

pub(super) fn admit(selected: &super::library::Selection, warning: Option<&str>) -> Result<()> {
    if selected.tracks.iter().any(|track| track.radio.is_some())
        && warning == Some("queue_restore_failed")
    {
        return Err("PLAYBACK_PERSISTENCE_FAILED");
    }
    Ok(())
}
