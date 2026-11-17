//! Shared PlaylistStore on a serial worker; Windows-only catalog/playback adapters.
use crate::{
    audio::MediaLocator,
    playlists::{NewPlaylistItem, Playlist, PlaylistStore},
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
pub type Result<T> = std::result::Result<T, &'static str>;
fn error(error: String) -> &'static str {
    if error == crate::playlists::DUPLICATE_PLAYLIST_NAME_ERROR {
        "playlist_name_duplicate"
    } else if error == "windows_playlist_limit" {
        "windows_playlist_limit"
    } else if error == "Playlist não encontrada." || error == "Item da playlist não encontrado." {
        "windows_playlist_missing"
    } else {
        "windows_playlist_failed"
    }
}
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    let v = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or("invalid_argument")?;
    if v.len() > 4096 || v.contains('\0') {
        return Err("invalid_argument");
    }
    Ok(v)
}
fn value<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T> {
    serde_json::from_value(args.get(key).cloned().ok_or("invalid_argument")?)
        .map_err(|_| "invalid_argument")
}
fn local_item(item: &mut NewPlaylistItem) -> Result<()> {
    if matches!(&item.media_locator,MediaLocator::Plex{..}) {
        let source=crate::windows_mpd::queue_v3::Source::from_locator(&item.media_locator).map_err(|_|"PLEX_INVALID")?;
        crate::windows_plex::validate_source(&source)?;
        if let MediaLocator::Plex{file_path,..}=&mut item.media_locator{*file_path=None;}
        if [&item.metadata.title,&item.metadata.artist,&item.metadata.album].iter().any(|v|v.len()>16384||v.contains('\0')){return Err("windows_playlist_limit");}
        return Ok(());
    }
    if matches!(&item.media_locator,MediaLocator::Navidrome{..}) {
        if crate::navidrome::storage_locator(&item.media_locator).is_none(){return Err("NAVIDROME_INVALID");}
        if [&item.metadata.title,&item.metadata.artist,&item.metadata.album].iter().any(|v|v.len()>16384){return Err("windows_playlist_limit");}
        return Ok(());
    }
    let MediaLocator::Local { uri } = &item.media_locator else {
        return Err("windows_feature_unavailable");
    };
    let parts: Vec<_> = uri.split('/').collect();
    let hash = |v: &str| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit());
    if parts.len() != 4
        || parts[0] != "windows"
        || !hash(parts[1])
        || !hash(parts[3])
        || !parts[2]
            .parse::<u32>()
            .is_ok_and(|n| n.to_string() == parts[2])
    {
        return Err("invalid_argument");
    }
    if [
        &item.metadata.title,
        &item.metadata.artist,
        &item.metadata.album,
    ]
    .iter()
    .any(|v| v.len() > 16384)
    {
        return Err("windows_playlist_limit");
    }
    Ok(())
}
fn items(args: &Value, key: &str) -> Result<Vec<NewPlaylistItem>> {
    let mut list: Vec<NewPlaylistItem> = value(args, key)?;
    if list.len() > 1000 {
        return Err("windows_playlist_limit");
    }
    for row in &mut list {
        local_item(row)?;
    }
    Ok(list)
}
pub fn dispatch(store: &PlaylistStore, operation: &str, args: &Value) -> Result<Value> {
    let encoded = match operation {
        "list_playlists" => serde_json::to_value(store.list().map_err(error)?),
        "get_playlist" => serde_json::to_value(store.get(text(args, "id")?).map_err(error)?),
        "create_playlist" => {
            serde_json::to_value(store.create(text(args, "name")?).map_err(error)?)
        }
        "create_playlist_with_items" => serde_json::to_value(
            store
                .create_with_items(text(args, "name")?, items(args, "items")?)
                .map_err(error)?,
        ),
        "rename_playlist" => serde_json::to_value(
            store
                .rename(text(args, "id")?, text(args, "name")?)
                .map_err(error)?,
        ),
        "delete_playlist" => {
            store.delete(text(args, "id")?).map_err(error)?;
            Ok(Value::Null)
        }
        "add_playlist_item" | "add_playlist_items" => {
            let id = text(args, "playlistId")?;
            let added = if operation == "add_playlist_item" {
                let mut item: NewPlaylistItem = value(args, "item")?;
                local_item(&mut item)?;
                vec![item]
            } else {
                items(args, "items")?
            };
            if store.get(id).map_err(error)?.items.len() + added.len() > 1000 {
                return Err("windows_playlist_limit");
            }
            let updated = if operation == "add_playlist_item" {
                store.add_item(id, added.into_iter().next().ok_or("invalid_argument")?)
            } else {
                store.add_items(id, added)
            };
            serde_json::to_value(updated.map_err(error)?)
        }
        "remove_playlist_item" => serde_json::to_value(
            store
                .remove_item(text(args, "playlistId")?, text(args, "itemId")?)
                .map_err(error)?,
        ),
        "reorder_playlist_items" => {
            let ids: Vec<String> = value(args, "orderedItemIds")?;
            if ids.len() > 1000 {
                return Err("windows_playlist_limit");
            }
            serde_json::to_value(
                store
                    .reorder_items(text(args, "playlistId")?, &ids)
                    .map_err(error)?,
            )
        }
        _ => return Err("windows_feature_unavailable"),
    };
    encoded.map_err(|_| "windows_playlist_failed")
}
struct Message {
    request: Option<(String, Value)>,
    reply: mpsc::Sender<Result<Value>>,
}
#[derive(Clone)]
pub struct Controller {
    sender: mpsc::SyncSender<Message>,
    stopping: Arc<AtomicBool>,
}
impl Controller {
    pub fn new(path: PathBuf) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Message>(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stop = stopping.clone();
        std::thread::Builder::new()
            .name("sonante-windows-playlists".into())
            .spawn(move || {
                let lock_path = path.with_extension("lock");
                let store = PlaylistStore::new(path);
                while let Ok(message) = receiver.recv() {
                    let Some((operation, args)) = message.request else {
                        let _ = message.reply.send(Ok(Value::Null));
                        break;
                    };
                    let result = if worker_stop.load(Ordering::Acquire) {
                        Err("shutting_down")
                    } else {
                        match crate::persistence::operation_lock(&lock_path) {
                            Ok(_guard) => dispatch(&store, &operation, &args),
                            Err(_) => Err("windows_playlist_busy"),
                        }
                    };
                    if message.reply.send(result).is_err() {
                        eprintln!("Windows playlists: operation_receiver_closed");
                    }
                }
            })
            .map_err(|_| "windows_playlist_failed")?;
        Ok(Self { sender, stopping })
    }
    pub fn call(&self, operation: &str, args: Value) -> Result<Value> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message {
                request: Some((operation.into(), args)),
                reply,
            })
            .map_err(|_| "windows_playlist_busy")?;
        receiver.recv().map_err(|_| "windows_playlist_failed")?
    }
    pub fn get(&self, id: &str) -> Result<Playlist> {
        serde_json::from_value(self.call("get_playlist", json!({"id":id}))?)
            .map_err(|_| "windows_playlist_failed")
    }
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message {
                request: None,
                reply,
            })
            .map_err(|_| "windows_playlist_busy")?;
        receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "windows_playlist_failed")??;
        Ok(())
    }
}
#[derive(Debug)]
pub struct PlaybackPlan {
    pub indices: Vec<usize>,
    pub start: usize,
    pub skipped: usize,
}
pub fn prepare(
    playlist: &Playlist,
    available: &[bool],
    start_item_id: Option<&str>,
    shuffle: bool,
    mut random: impl FnMut(usize) -> Result<usize>,
) -> Result<PlaybackPlan> {
    if playlist.items.len() > 1000 {
        return Err("windows_playlist_limit");
    }
    if available.len() != playlist.items.len() || shuffle && start_item_id.is_some() {
        return Err("invalid_argument");
    }
    let chosen = match start_item_id {
        Some(id) => Some(
            playlist
                .items
                .iter()
                .position(|i| i.id == id)
                .ok_or("windows_playlist_missing")?,
        ),
        None => None,
    };
    if chosen.is_some_and(|i| !available[i]) {
        return Err("playlist_selected_item_unavailable");
    }
    let mut indices: Vec<_> = available
        .iter()
        .enumerate()
        .filter(|(_, yes)| **yes)
        .map(|(i, _)| i)
        .collect();
    if indices.is_empty() {
        return Err("playlist_no_playable_items");
    }
    let mut start = chosen
        .and_then(|i| indices.iter().position(|j| *j == i))
        .unwrap_or(0);
    if shuffle {
        for end in (1..indices.len()).rev() {
            let i = random(end + 1)?;
            if i > end {
                return Err("windows_playlist_failed");
            }
            indices.swap(i, end);
        }
        start = 0;
    }
    Ok(PlaybackPlan {
        skipped: playlist.items.len() - indices.len(),
        indices,
        start,
    })
}
pub fn random_index(upper: usize) -> Result<usize> {
    if upper == 0 {
        return Err("invalid_argument");
    }
    let upper = upper as u64;
    let threshold = upper.wrapping_neg() % upper;
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|_| "random_failed")?;
        let v = u64::from_ne_bytes(bytes);
        if v >= threshold {
            return Ok((v % upper) as usize);
        }
    }
}
#[cfg(test)]
#[path = "windows_playlists_tests.rs"]
mod tests;

#[derive(serde::Serialize, Debug)]
pub struct Availability {
    pub item_id: String,
    pub status: &'static str,
    pub reason: Option<&'static str>,
}
pub fn resolve(
    playlist: &Playlist,
    library: &crate::windows_mpd::library::Library,
) -> Result<Vec<Availability>> {
    let playlist=playlist.clone();let library=library.clone();
    crate::windows_io::read(Duration::from_secs(6),move|job|resolve_read(&playlist,&library,||job.current()))
        .map_err(|e|match e{crate::windows_io::Error::Busy=>"engine_busy",crate::windows_io::Error::Timeout=>"index_timeout",crate::windows_io::Error::WorkerFailed=>"worker_failed"})?
}
fn resolve_read(playlist:&Playlist,library:&crate::windows_mpd::library::Library,current:impl Fn()->bool)->Result<Vec<Availability>>{
    use std::fs;
    if playlist.items.len() > 1000 {
        return Err("windows_playlist_limit");
    }
    let before = library.status()?;
    let sources=library.sources()?;
    let current=||current()&&library.current(before.epoch);
    let mut missing_files=Vec::new();let mut missing_catalog=Vec::new();
    let mut rows = Vec::new();
    for item in &playlist.items {
        if !current(){return Err("index_timeout");}
        let mut row = Availability {
            item_id: item.id.clone(),
            status: "unavailable",
            reason: Some("library_unavailable"),
        };
        if let MediaLocator::Local { uri } = &item.media_locator {
            if sources.local&&before.phase != "failed" {
                match library.compatibility_selection(&[uri.clone()], 0) {
                    Ok(selected) => {
                        let path = &selected.tracks[0].path;
                        match fs::symlink_metadata(path) {
                            Ok(m) if m.is_file() && !crate::persistence::linked(&m) => {
                                if !current(){return Err("index_timeout");}
                                // The same held ancestor/reparse/SMB identity
                                // checks as playback, without reading content.
                                if crate::windows_file_info::with_bound_file_while(&selected.root,path,&current,|_,_|Ok(())).is_ok(){
                                    row.status = "available";
                                    row.reason = None;
                                }
                            }
                            // A replaced directory/reparse/cloud placeholder
                            // does not establish that a catalog file is absent.
                            Ok(_) => {},
                            Err(e) if definite_absence(&e)=>missing_files.push((rows.len(),selected.root.clone(),path.clone())),
                            Err(_) => {},
                        }
                    }
                    Err("library_changed") if before.phase == "ready" && before.warnings == 0 =>missing_catalog.push(rows.len()),
                    Err(_) => {}
                }
            }
        } else {
            row.reason = Some("windows_feature_unavailable");
        }
        rows.push(row);
    }
    if !current(){return Err("index_timeout");}
    // One live-root proof for this bounded request, not a new worker or a
    // per-item six-second wait. Failure keeps every candidate unavailable.
    if !missing_files.is_empty()||!missing_catalog.is_empty(){
        if let Ok(proof)=library.missing_roots_read(before.epoch,before.revision,&current){
            if !current(){return Err("index_timeout");}proof.validate(library)?;
            // A known catalog file with an explicit file/path-not-found result
            // is distinct from an unknown catalog URI. Unrelated tag warnings
            // do not resurrect a proven absent file, but cannot prove an
            // unknown album/track missing from a partial catalog.
            for(index,root,path)in missing_files{
                if !current(){return Err("index_timeout");}
                let absent=path.parent().is_some_and(|parent|crate::windows_file_info::ContentDirectory::observe(&root,parent,&current,||{
                    Ok(fs::symlink_metadata(&path).is_err_and(|error|definite_absence(&error)))
                }).unwrap_or(false));
                if absent{rows[index].status="missing";rows[index].reason=None;}
            }
            if proof.clean_catalog(){for index in missing_catalog{rows[index].status="missing";rows[index].reason=None;}}
            proof.validate(library)?;
        }
    }
    if !current(){return Err("index_timeout");}
    let after=library.status()?;
    if after.epoch != before.epoch||after.revision!=before.revision||library.sources()?.local!=sources.local {
        return Err("library_changed");
    }
    Ok(rows)
}
fn windows_absence_code(code:Option<i32>)->bool{matches!(code,Some(2|3))}
fn definite_absence(error:&std::io::Error)->bool{
    if error.kind()!=std::io::ErrorKind::NotFound{return false;}
    #[cfg(windows)]{windows_absence_code(error.raw_os_error())}
    #[cfg(not(windows))]{true}
}

// Called on the existing blocking bridge/store worker, never a GUI or Tokio
// executor thread. Probing a remote item does not open the media proxy.
pub(crate) fn resolve_sources(playlist:&Playlist,library:&crate::windows_mpd::library::Library,navidrome:&crate::windows_navidrome::Controller)->Result<Vec<Availability>> {
    let mut rows=resolve(playlist,library)?;
    let sources=library.sources()?;
    let mut probes=Vec::new();
    for (index,item) in playlist.items.iter().enumerate(){
        match &item.media_locator {
            MediaLocator::Local{..} if !sources.local => {rows[index].status="unavailable";rows[index].reason=Some("windows_source_disabled");},
            MediaLocator::Navidrome{..} if sources.navidrome => probes.push((index,item.media_locator.clone())),
            MediaLocator::Navidrome{..} => {rows[index].status="unavailable";rows[index].reason=Some("NAVIDROME_CONTEXT_CHANGED");},
            _=>{},
        }
    }
    let results=tauri::async_runtime::block_on(navidrome.available_many(probes));
    for (index,value) in results {
        match value {
            Ok(true)=>{rows[index].status="available";rows[index].reason=None;},
            Ok(false)=>{rows[index].status="missing";rows[index].reason=None;},
            Err(error)=>{rows[index].status="unavailable";rows[index].reason=Some(crate::windows_navidrome::playback_error(&error));},
        }
    }
    Ok(rows)
}

/// Resolve all persisted source kinds without mutating or discarding missing rows.
pub(crate) fn resolve_all(playlist:&Playlist,library:&crate::windows_mpd::library::Library,navidrome:&crate::windows_navidrome::Controller,plex:&crate::windows_plex::Controller)->Result<Vec<Availability>>{
 let mut rows=resolve_sources(playlist,library,navidrome)?;
 let sources=library.sources()?;let mut probes=Vec::new();
 for(index,item)in playlist.items.iter().enumerate(){if matches!(item.media_locator,MediaLocator::Plex{..}){
  if sources.plex{probes.push((index,item.media_locator.clone()));}else{rows[index].status="unavailable";rows[index].reason=Some("PLEX_CONTEXT_CHANGED");}
 }}
 for(index,result)in tauri::async_runtime::block_on(plex.available_many(probes)){
  match result{
   Ok(Some(true))=>{rows[index].status="available";rows[index].reason=None;},
   Ok(Some(false))=>{rows[index].status="missing";rows[index].reason=None;},
   Ok(None)=>{rows[index].status="unavailable";rows[index].reason=Some("PLEX_UNAVAILABLE");},
   Err(error)=>{rows[index].status="unavailable";rows[index].reason=Some(crate::windows_plex::playback_error(&error));},
  }
 }Ok(rows)
}
