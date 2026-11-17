//! Navidrome playback preparation is outside the MPD foreground worker. Durable
//! metadata contains account/song identity only; each occurrence owns its lease.
use super::{api, connection::Registry, host::Controller, locator::Resolved, CHANGED, INVALID, UNAVAILABLE};
use crate::windows_mpd::{library::{RemoteLease, Selection, Track}, queue_v3::Source};
use crate::audio::MediaLocator;
use std::sync::Arc;

fn code(value: String) -> &'static str {
    match value.as_str() {
        super::AUTH => super::AUTH, super::STORAGE => super::STORAGE,
        INVALID => INVALID, CHANGED => CHANGED, super::PROTOCOL => super::PROTOCOL,
        _ => UNAVAILABLE,
    }
}
struct Lease { registry: Arc<Registry>, resolved: Resolved, source: Source }
impl RemoteLease for Lease {
    fn uri(&self) -> &str { &self.resolved.uri }
    fn source(&self) -> &Source { &self.source }
    fn connection_id(&self) -> &str {
        let MediaLocator::Navidrome { connection_id, .. }=&self.resolved.locator else { unreachable!() }; connection_id
    }
    fn presentation_duration(&self)->Option<f64>{self.resolved.duration}
    fn healthy(&self) -> bool { self.resolved.active() }
    fn health_signal(&self)->Option<std::sync::Arc<std::sync::atomic::AtomicBool>>{Some(self.resolved.health_signal())}
    fn command(&self, work: &mut dyn FnMut()->crate::windows_mpd::protocol::Result<Vec<String>>) -> crate::windows_mpd::protocol::Result<Vec<String>> {
        self.registry.with_resolved(std::slice::from_ref(&self.resolved), || work().map_err(str::to_owned)).map_err(code)
    }
}
impl Controller {
    pub(crate) fn contextualize(&self, tracks: &mut [Track]) {
        for track in tracks {
            if let Some(Source::Navidrome { account_id, .. })=&track.remote_source {
                if let Ok(connection)=self.registry().for_account(account_id) {track.remote_connection_id=Some(connection.credentials.connection_id.clone());}
                else if track.remote_lease.is_none() {track.remote_connection_id=None;}
            }
        }
    }
    pub(crate) async fn prepare_tracks(&self, mut tracks: Vec<Track>) -> Result<Vec<Track>,String> {
        if tracks.is_empty()||tracks.len()>1000 { return Err(INVALID.into()); }
        // Retained only for already bound, exact occurrences. Reopening a disk
        // queue has no leases and must resolve against the current account.
        let registry=self.registry();
        self.read(async {
            let mut work=std::collections::VecDeque::new();
            for (index,track) in tracks.iter_mut().enumerate() {
                let Some(source)=track.remote_source.as_ref() else { continue };
                if track.radio.is_some()||!track.path.as_os_str().is_empty()||track.media_root.is_some() { return Err(INVALID.into()); }
                source.validate().map_err(str::to_owned)?;
                let Source::Navidrome {account_id,song_id}=source else { return Err("windows_feature_unavailable".into()); };
                if let Some(lease)=&track.remote_lease {
                    if lease.source()==source&&lease.healthy() {track.remote_connection_id=Some(lease.connection_id().to_owned());continue;}
                }
                work.push_back((index,MediaLocator::Navidrome{connection_id:String::new(),account_id:Some(account_id.clone()),song_id:song_id.clone()}));
            }
            let mut pending=tokio::task::JoinSet::new();
            loop {
                while pending.len()<4 {
                    let Some((index,locator))=work.pop_front() else { break };
                    let registry=registry.clone();
                    pending.spawn(async move {registry.resolve_saved(&locator).await.map(|resolved|(index,registry,resolved))});
                }
                let Some(item)=pending.join_next().await else {break};
                let (index,registry,resolved)=item.map_err(|_|UNAVAILABLE)??;
                let source=tracks[index].remote_source.clone().ok_or(INVALID)?;
                tracks[index].remote_connection_id=match &resolved.locator {MediaLocator::Navidrome{connection_id,..}=>Some(connection_id.clone()),_=>return Err(INVALID.into())};
                tracks[index].remote_lease=Some(Arc::new(Lease{registry,resolved,source}));
            }
            for track in &tracks {
                if let Some(source)=&track.remote_source {
                    let lease=track.remote_lease.as_ref().ok_or(CHANGED)?;
                    if lease.source()!=source||!lease.healthy(){return Err(CHANGED.into());}
                }
            }
            Ok(tracks)
        }).await
    }
    pub(crate) fn prepared_album(&self, connection_id:&str,id:&str,start:usize)->Result<Selection,String> {
        if !api::id(id){return Err(INVALID.into());}
        let registry=self.registry();let connection=registry.current(connection_id)?;
        let detail=registry.prepared(connection_id,id)?;
        if detail.songs.is_empty()||detail.songs.len()>500||start>=detail.songs.len(){return Err(INVALID.into());}
        registry.with_current(&connection,||{
            let mut tracks=Vec::new();
            for song in detail.songs {
                let source=Source::Navidrome{account_id:connection.account(),song_id:song.id.clone()};
                let locator=MediaLocator::Navidrome{connection_id:connection_id.to_owned(),account_id:Some(connection.account()),song_id:song.id.clone()};
                let resolved=Resolved::prepared(connection.clone(),locator,connection.stream(&song.id)?,song.duration)?;
                tracks.push(Track{id:song.id,title:song.title,artist:song.artist,album:song.album,disc:song.disc,number:song.track,file_name:String::new(),catalog_album_id:None,path:Default::default(),media_root:None,radio:None,upnp:None,remote_source:Some(source.clone()),remote_connection_id:Some(connection_id.to_owned()),plex_image:None,remote_lease:Some(Arc::new(Lease{registry:registry.clone(),resolved,source}))});
            }
            Ok(Selection{root:Default::default(),tracks,start,album_id:None})
        })
    }
    pub(crate) fn bind_locator(&self, locator:&MediaLocator)->Result<Source,String> {
        let MediaLocator::Navidrome{connection_id,account_id,song_id}=locator else {return Err(INVALID.into())};
        if !api::id(song_id){return Err(INVALID.into());}
        let c=match account_id {Some(account)=>self.registry().for_account(account)?,None=>self.registry().current(connection_id)?};
        Ok(Source::Navidrome{account_id:c.account(),song_id:song_id.clone()})
    }
}

impl Controller {
    async fn available(&self,locator:MediaLocator)->Result<bool,String>{
        let source=self.bind_locator(&locator)?;
        let Source::Navidrome{account_id,song_id}=source else{return Err(INVALID.into())};
        let registry=self.registry();let connection=registry.for_account(&account_id)?;
        self.read(async {
            let root=connection.request("getSong",&[("id",song_id.clone())]).await?;
            root.get("song").filter(|value|value["id"].as_str()==Some(song_id.as_str())).ok_or(super::PROTOCOL)?;
            registry.with_current(&connection,||Ok(true))
        }).await
    }
    pub(crate) async fn available_many(&self,items:Vec<(usize,MediaLocator)>)->Vec<(usize,Result<bool,String>)>{
        if items.len()>1000{return items.into_iter().map(|(i,_)|(i,Err(INVALID.into()))).collect();}
        let mut todo=std::collections::VecDeque::from(items);let mut pending=tokio::task::JoinSet::new();let mut result=Vec::new();
        // A single bounded probe round cannot keep the playlist view waiting
        // indefinitely. Unknown items remain unavailable, never deleted/missing.
        let all_indices=todo.iter().map(|(i,_)|*i).collect::<Vec<_>>();
        let round=async {
            loop {
                while pending.len()<4 {let Some((index,locator))=todo.pop_front()else{break};let host=self.clone();pending.spawn(async move{(index,host.available(locator).await)});}
                let Some(value)=pending.join_next().await else{break};
                match value {Ok(value)=>result.push(value),Err(_)=>break}
            }
        };
        let _complete=tokio::time::timeout(std::time::Duration::from_secs(16),round).await;
        pending.abort_all();
        for index in all_indices {if !result.iter().any(|(seen,_)|*seen==index){result.push((index,Err(UNAVAILABLE.into())));}}
        result
    }
}
