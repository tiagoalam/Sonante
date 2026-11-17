use super::*;
fn wave(path: &Path, title: &str) {
    let mut body = b"WAVEfmt ".to_vec();
    body.extend(16u32.to_le_bytes());
    body.extend([1, 0, 2, 0]);
    body.extend(44100u32.to_le_bytes());
    body.extend(176400u32.to_le_bytes());
    body.extend([4, 0, 16, 0]);
    body.extend(b"data");
    body.extend(176400u32.to_le_bytes());
    body.extend(vec![0; 176400]);
    let mut info = b"INFO".to_vec();
    for (key, value) in [(b"INAM", title), (b"IPRD", "Álbum"), (b"IART", "Artista")] {
        let mut value = value.as_bytes().to_vec();
        value.push(0);
        info.extend(key);
        info.extend((value.len() as u32).to_le_bytes());
        info.extend(&value);
        if value.len() % 2 != 0 {
            info.push(0);
        }
    }
    body.extend(b"LIST");
    body.extend((info.len() as u32).to_le_bytes());
    body.extend(info);
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    fs::write(path, file).unwrap();
}
struct Fixture{base:PathBuf,library:Library,track:Track}
impl Fixture{fn new()->Self{let mut n=[0;12];getrandom::fill(&mut n).unwrap();let base=std::env::temp_dir().join(format!("sonante-own-queue-presentation-{n:x?}"));let music=base.join("Music");fs::create_dir_all(&music).unwrap();wave(&music.join("one.wav"),"Own fixture");let library=Library::new(base.join("state/folders.json"));library.select(music).unwrap();let end=std::time::Instant::now()+std::time::Duration::from_secs(3);while library.status().unwrap().phase=="scanning"{assert!(std::time::Instant::now()<end);std::thread::sleep(std::time::Duration::from_millis(2));}let track=library.data.lock().unwrap().albums.values().next().unwrap().tracks[0].clone();Self{base,library,track}}}
impl Drop for Fixture{fn drop(&mut self){self.library.stop();fs::remove_dir_all(&self.base).unwrap();}}
struct Lease{source:super::super::super::queue_v3::Source,seconds:Option<f64>,live:bool}
impl RemoteLease for Lease {
 fn uri(&self)->&str{panic!("duration must not read a media capability")}
 fn source(&self)->&super::super::super::queue_v3::Source{&self.source}
 fn connection_id(&self)->&str{panic!("no connection lookup")}
 fn healthy(&self)->bool{self.live}
 fn presentation_duration(&self)->Option<f64>{self.seconds}
 fn command(&self,_:&mut dyn FnMut()->Result<Vec<String>>)->Result<Vec<String>>{panic!("no engine/network effect")}
}
fn remote(track:&Track,source:super::super::super::queue_v3::Source,seconds:Option<f64>,live:bool)->Track{let mut t=track.clone();t.remote_source=Some(source.clone());t.remote_lease=Some(Arc::new(Lease{source,seconds,live}));t}
fn nav()->super::super::super::queue_v3::Source{super::super::super::queue_v3::Source::Navidrome{account_id:"a".repeat(64),song_id:"song".into()}}
fn plex()->super::super::super::queue_v3::Source{super::super::super::queue_v3::Source::Plex{server_id:"server".into(),rating_key:"rating".into(),part_key:"/part".into()}}
#[test]fn actual_local_properties_and_bound_remote_metadata_follow_duplicate_occurrence_order(){let f=Fixture::new();let n=remote(&f.track,nav(),Some(120.25),true);let p=remote(&f.track,plex(),Some(123.456),true);let queue=vec![f.track.clone(),n.clone(),p.clone(),f.track.clone()];assert_eq!(f.library.queue_durations(&queue),[Some(1.),Some(120.25),Some(123.456),Some(1.)]);assert_eq!(f.library.queue_durations(&[p,n,f.track.clone()]),[Some(123.456),Some(120.25),Some(1.)]);let json=serde_json::to_value(queue).unwrap();assert!(json.as_array().unwrap().iter().all(|t|t.get("duration").is_none()));}
#[test]fn cached_local_properties_need_no_filesystem_read_and_do_not_change_source_or_settings(){let f=Fixture::new();let settings=fs::read(&f.library.config).unwrap();let journal=fs::read(added::path(&f.library.config)).unwrap();fs::remove_file(&f.track.path).unwrap();assert_eq!(f.library.queue_durations(&[f.track.clone()]),[Some(1.)]);assert_eq!(fs::read(&f.library.config).unwrap(),settings);assert_eq!(fs::read(added::path(&f.library.config)).unwrap(),journal);}
#[test]fn busy_catalog_is_unknown_without_blocking_or_downgrading_bound_remote_metadata(){let f=Fixture::new();let n=remote(&f.track,nav(),Some(120.),true);let _held=f.library.data.lock().unwrap();assert_eq!(f.library.queue_durations(&[f.track.clone(),n]),[None,Some(120.)]);}
#[test]fn stale_wrong_and_unresolved_remote_leases_stay_unknown(){let f=Fixture::new();let mut wrong=remote(&f.track,nav(),Some(120.),true);wrong.remote_source=Some(plex());let dead=remote(&f.track,nav(),Some(120.),false);let mut unresolved=dead.clone();unresolved.remote_lease=None;assert_eq!(f.library.queue_durations(&[wrong,dead,unresolved]),[None,None,None]);}
#[test]fn wrong_local_root_path_and_album_are_unknown_instead_of_reusing_a_track_title(){let f=Fixture::new();let mut wrong_root=f.track.clone();wrong_root.media_root=None;let mut wrong_path=f.track.clone();wrong_path.path=f.base.join("wrong.wav");let mut wrong_album=f.track.clone();wrong_album.catalog_album_id=Some("absent".into());assert_eq!(f.library.queue_durations(&[wrong_root,wrong_path,wrong_album]),[None,None,None]);}
#[test]fn radio_and_nonfinite_zero_negative_metadata_remain_unknown(){let f=Fixture::new();for value in [None,Some(0.),Some(-1.),Some(f64::NAN),Some(f64::INFINITY),Some(f64::NEG_INFINITY)]{assert_eq!(f.library.queue_durations(&[remote(&f.track,nav(),value,true)]),[None]);}let mut radio=remote(&f.track,nav(),Some(120.),true);radio.radio=Some(crate::windows_radio::Playing{station_id:"station".into(),thumb:None,stream_url:String::new(),station_name:"Own fixture".into()});assert_eq!(f.library.queue_durations(&[radio]),[None]);}
