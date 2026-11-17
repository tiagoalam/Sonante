//! Read-only source binding published by the existing worker, never a second poll.
use crate::windows_file_info::FileIdentity;
use super::library::{track_uri, Track};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
#[derive(Clone, PartialEq, Eq)]
struct Entry {
    uri: String,
    path: PathBuf,
}
struct Queue {
    generation: u64,
    index: usize,
    identity: Option<FileIdentity>,
    entries: Vec<Entry>,
}
#[derive(Clone)]
pub struct Binding {
    pub generation: u64,
    pub index: usize,
    pub uri: String,
    pub path: PathBuf,
    pub identity: Option<FileIdentity>,
    intent: u64,
}
#[derive(Default)]
pub struct View(Mutex<Option<Queue>>);
impl View {
    #[cfg(test)]
    pub fn publish(&self, generation: u64, running: bool, state: &str, tracks: &[Track], index: usize) {
        self.publish_bound(generation, running, state, tracks, index, None)
    }
    pub fn publish_bound(
        &self,
        generation: u64,
        running: bool,
        state: &str,
        tracks: &[Track],
        index: usize,
        identity: Option<FileIdentity>,
    ) {
        let Ok(mut value) = self.0.lock() else {
            eprintln!("Windows source facts: state_unavailable");
            return;
        };
        if !running || !["play", "pause"].contains(&state) || index >= tracks.len()
            || tracks.get(index).is_some_and(|track| track.radio.is_some() || track.upnp.is_some() || track.remote_source.is_some()) {
            *value = None;
            return;
        }
        if let Some(current) = value
            .as_mut()
            .filter(|v| v.generation == generation && v.entries.len() == tracks.len())
        {
            current.index = index;
            current.identity = identity;
            return;
        }
        // Presentation tags are not duplicated, and a normal status/volume tick
        // updates only the cursor. Queue generation fences structural changes.
        *value = Some(Queue {
            generation,
            index,
            identity,
            entries: tracks
                .iter()
                .map(|t| Entry {
                    uri: track_uri(t),
                    path: t.path.clone(),
                })
                .collect(),
        });
    }
    pub fn clear(&self) {
        if let Ok(mut value) = self.0.lock() {
            *value = None;
        }
    }
    pub fn capture(&self, uri: &str, index: usize, intent: u64) -> Option<Binding> {
        let value = self.0.lock().ok()?;
        let queue = value.as_ref()?;
        if queue.index != index {
            return None;
        }
        let entry = queue.entries.get(index)?;
        if entry.uri != uri {
            return None;
        }
        Some(Binding {
            generation: queue.generation,
            index,
            uri: entry.uri.clone(),
            path: entry.path.clone(),
            identity: queue.identity,
            intent,
        })
    }
    pub fn current(&self, binding: &Binding, intent: u64) -> bool {
        if binding.intent != intent {
            return false;
        }
        let Some(now) = self.capture(&binding.uri, binding.index, intent) else {
            return false;
        };
        now.generation == binding.generation && now.path == binding.path && now.identity == binding.identity
    }
}
pub struct ClearOnDrop(pub Arc<View>);
impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            title: "ignored presentation tag".into(),
            artist: String::new(),
            album: String::new(),
            disc: 1,
            number: 1,
            file_name: "a.flac".into(),
            catalog_album_id: Some("album".into()),
            media_root: None,
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
            path: PathBuf::from(format!("/catalog/{id}.flac")),
        }
    }
    #[test]
    fn only_confirmed_live_current_occurrence_can_bind_and_logical_startup_never_does() {
        let view = View::default();
        let tracks = vec![track("a"), track("a"), track("b")];
        let uri = track_uri(&tracks[0]);
        for (running, state) in [
            (false, "pause"),
            (false, "stop"),
            (true, "stop"),
            (true, "idle"),
        ] {
            view.publish(1, running, state, &tracks, 1);
            assert!(view.capture(&uri, 1, 1).is_none());
        }
        view.publish(1, true, "play", &tracks, 1);
        assert!(view.capture(&uri, 0, 1).is_none());
        assert!(view.capture("windows/forged", 1, 1).is_none());
        assert!(view.capture(&uri, 3, 1).is_none());
        let binding = view.capture(&uri, 1, 1).unwrap();
        assert_eq!(binding.index, 1);
        assert!(view.current(&binding, 1));
    }
    #[test]
    fn cursor_queue_intent_failure_and_shutdown_invalidate_old_source_without_transport() {
        let view = View::default();
        let tracks = vec![track("a"), track("b")];
        let uri = track_uri(&tracks[0]);
        view.publish(1, true, "play", &tracks, 0);
        let binding = view.capture(&uri, 0, 4).unwrap();
        assert!(!view.current(&binding, 5));
        view.publish(1, true, "pause", &tracks, 0);
        assert!(view.current(&binding, 4));
        view.publish(1, true, "play", &tracks, 1);
        assert!(!view.current(&binding, 4));
        view.publish(2, true, "play", &tracks, 0);
        assert!(!view.current(&binding, 4));
        let current = view.capture(&uri, 0, 4).unwrap();
        view.clear();
        assert!(!view.current(&current, 4));
        view.publish(3, true, "play", &tracks, 0);
        let current = view.capture(&uri, 0, 4).unwrap();
        let shared = Arc::new(view);
        drop(ClearOnDrop(shared.clone()));
        assert!(!shared.current(&current, 4));
    }
    #[test]
    fn exact_media_identity_is_part_of_the_current_binding_not_a_path_only_claim() {
        let view = View::default(); let tracks = vec![track("a")]; let uri = track_uri(&tracks[0]);
        view.publish_bound(1,true,"play",&tracks,0,Some(FileIdentity::Id128([1;24])));
        let binding = view.capture(&uri,0,0).unwrap(); assert_eq!(binding.identity,Some(FileIdentity::Id128([1;24])));
        view.publish_bound(1,true,"pause",&tracks,0,Some(FileIdentity::Id128([2;24])));
        assert!(!view.current(&binding,0));
        view.publish_bound(1,true,"play",&tracks,0,Some(FileIdentity::Smb64{volume:1,index:1}));
        let legacy=view.capture(&uri,0,0).unwrap();
        view.publish_bound(1,true,"play",&tracks,0,Some(FileIdentity::Id128([1;24])));
        assert!(!view.current(&legacy,0));
        view.publish_bound(1,true,"play",&tracks,0,None);
        assert_eq!(view.capture(&uri,0,0).unwrap().identity,None);
    }
    #[test]
    fn status_ticks_reuse_binding_entries_but_generation_replacement_updates_physical_path() {
        let view = View::default();
        let tracks = vec![track("a")];
        let uri = track_uri(&tracks[0]);
        view.publish(1, true, "play", &tracks, 0);
        let binding = view.capture(&uri, 0, 0).unwrap();
        let allocation = view.0.lock().unwrap().as_ref().unwrap().entries.as_ptr() as usize;
        for _ in 0..100 {
            view.publish(1, true, "pause", &tracks, 0);
            assert_eq!(
                view.0.lock().unwrap().as_ref().unwrap().entries.as_ptr() as usize,
                allocation
            );
        }
        let mut replacement = tracks;
        replacement[0].path = PathBuf::from("/different-root/a.flac");
        view.publish(2, true, "play", &replacement, 0);
        assert!(!view.current(&binding, 0));
        assert_eq!(view.capture(&uri, 0, 0).unwrap().path, replacement[0].path);
    }
}

#[cfg(test)]
mod radio_boundary_tests {
 use super::*;
 #[test]
 fn transient_station_clears_local_source_binding_without_reading_a_path(){
  let selected=crate::windows_radio::Station{id:format!("manual:{}","a".repeat(64)),name:"Fixture".into(),country_code:String::new(),tags:vec![],codec:String::new(),bitrate:0,favicon:None,stream_url:"https://fixture.invalid/?key=PRIVATE".into(),manual:true}.selection().unwrap();
  let view=View::default();view.publish_bound(2,true,"play",&selected.tracks,0,None);
  assert!(view.capture(&track_uri(&selected.tracks[0]),0,1).is_none());
 }
}
