//! Original source-fact schema; Windows queue binding + original header inspection.
use crate::local_file_info::LocalFileInfo;
use serde::Serialize;
#[derive(Clone, Serialize)]
pub struct Media {
    kind: &'static str,
    pub uri: String,
    pub queue_index: usize,
}
impl Media {
    pub fn decode(value: &serde_json::Value) -> Option<Self> {
        if value.get("kind")?.as_str()? != "local" {
            return None;
        }
        let uri = value.get("uri")?.as_str()?;
        if uri.len() > 512 {
            return None;
        }
        let queue_index = usize::try_from(value.get("queue_index")?.as_u64()?).ok()?;
        if queue_index >= 1000 {
            return None;
        }
        Some(Self {
            kind: "local",
            uri: uri.into(),
            queue_index,
        })
    }
}
#[derive(Serialize)]
pub struct Facts {
    provenance: &'static str,
    codec: &'static str,
    compression: &'static str,
    sample_type: &'static str,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    dsd_level: Option<u16>,
    revision: Option<String>,
}
impl From<LocalFileInfo> for Facts {
    fn from(info: LocalFileInfo) -> Self {
        Self {
            provenance: "local",
            codec: info.codec,
            compression: info.compression,
            sample_type: info.sample_type,
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            dsd_level: info.dsd_level,
            revision: Some(info.revision),
        }
    }
}
#[derive(Debug,Serialize)]
pub struct RemoteFacts {
    pub provenance:&'static str,pub codec:&'static str,pub compression:&'static str,
    pub sample_type:&'static str,pub sample_rate:Option<u32>,pub bit_depth:Option<u8>,
    pub dsd_level:Option<u16>,pub revision:Option<String>,
}
#[derive(Clone,PartialEq,Eq)]
pub(crate) struct RemoteBinding{pub generation:u64,pub revision:String,pub index:usize,pub source:crate::windows_mpd::queue_v3::Source}
#[derive(Clone,Serialize)]
pub(crate) struct RemoteMedia{kind:&'static str,pub server_id:String,pub part_key:String,pub queue_index:usize}
impl RemoteMedia{
 pub fn decode(value:&serde_json::Value)->Option<Self>{
  if value.get("kind")?.as_str()?!="plex"{return None;}
  let server_id=value.get("server_id")?.as_str()?;let part_key=value.get("part_key")?.as_str()?;
  let queue_index=usize::try_from(value.get("queue_index")?.as_u64()?).ok()?;
  if queue_index>=1000||server_id.len()>256||part_key.len()>4096{return None;}
  Some(Self{kind:"plex",server_id:server_id.into(),part_key:part_key.into(),queue_index})
 }
 pub fn matches(&self,binding:&RemoteBinding)->bool{matches!(&binding.source,crate::windows_mpd::queue_v3::Source::Plex{server_id,part_key,..}if self.queue_index==binding.index&&self.server_id==*server_id&&self.part_key==*part_key)}
}
pub(crate) struct RemotePermit;
static REMOTE_ACTIVE:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
impl RemotePermit{pub fn acquire()->Option<Self>{REMOTE_ACTIVE.compare_exchange(false,true,std::sync::atomic::Ordering::AcqRel,std::sync::atomic::Ordering::Acquire).ok().map(|_|Self)}}
impl Drop for RemotePermit{fn drop(&mut self){REMOTE_ACTIVE.store(false,std::sync::atomic::Ordering::Release);}}
#[derive(Serialize)]
pub struct Frame {
    pub media: Media,
    pub info: Facts,
}
pub fn acquire(
    media: Media,
    current: impl Fn() -> bool,
    read: impl FnOnce() -> Result<LocalFileInfo, crate::local_file_info::InfoError>,
) -> Option<Frame> {
    if !current() {
        return None;
    }
    let info = read().ok()?;
    if !current() {
        return None;
    }
    Some(Frame {
        media,
        info: Facts::from(info),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_remote_or_out_of_bounds_media_is_not_authority() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!({"kind":"plex","uri":"x","queue_index":0}),
            serde_json::json!({"kind":"local","uri":"x","queue_index":-1}),
            serde_json::json!({"kind":"local","uri":"x","queue_index":1000}),
            serde_json::json!({"kind":"local","uri":"x".repeat(513),"queue_index":0}),
        ] {
            assert!(Media::decode(&value).is_none());
        }
        assert_eq!(
            Media::decode(
                &serde_json::json!({"kind":"local","uri":"windows/opaque","queue_index":3})
            )
            .unwrap()
            .queue_index,
            3
        );
    }
    #[test]
    fn original_fact_projection_preserves_source_fields_without_output_claims_or_paths() {
        let mut file = crate::local_file_info::read_streaminfo(&mut std::io::Cursor::new(
            crate::local_file_info::tests::fixture(96000, 24, 2, 96000),
        ))
        .unwrap();
        file.revision = "opaque-revision".into();
        let facts = serde_json::to_value(Facts::from(file)).unwrap();
        assert_eq!(
            facts,
            serde_json::json!({"provenance":"local","codec":"FLAC","compression":"lossless","sample_type":"integer","sample_rate":96000,"bit_depth":24,"dsd_level":null,"revision":"opaque-revision"})
        );
        assert!(facts.get("output").is_none());
        assert!(facts.get("path").is_none());
        assert!(facts.get("bitPerfect").is_none());
    }
    #[test]
    fn obsolete_or_failed_read_never_publishes_and_invalid_before_read_does_no_io() {
        use std::cell::Cell;
        let media = || {
            Media::decode(
                &serde_json::json!({"kind":"local","uri":"windows/opaque","queue_index":0}),
            )
            .unwrap()
        };
        let file = || {
            crate::local_file_info::read_streaminfo(&mut std::io::Cursor::new(
                crate::local_file_info::tests::fixture(44100, 16, 2, 44100),
            ))
        };
        assert!(acquire(
            media(),
            || false,
            || panic!("obsolete input must never read media")
        )
        .is_none());
        let current = Cell::new(true);
        let reads = Cell::new(0);
        assert!(acquire(
            media(),
            || current.get(),
            || {
                reads.set(reads.get() + 1);
                current.set(false);
                file()
            }
        )
        .is_none());
        assert_eq!(reads.get(), 1);
        assert!(acquire(
            media(),
            || true,
            || Err(crate::local_file_info::InfoError::UnsupportedFormat)
        )
        .is_none());
        assert!(acquire(media(), || true, file).is_some());
    }
}

#[cfg(test)]
mod remote_tests{
 use super::*;
 #[test]
 fn remote_media_cannot_bind_another_occurrence_or_server_and_never_admits_nav(){
  let value=serde_json::json!({"kind":"plex","server_id":"server-one","part_key":"/library/parts/1/file.flac","queue_index":3});let media=RemoteMedia::decode(&value).unwrap();
  let mut binding=RemoteBinding{generation:1,revision:"revision".into(),index:3,source:crate::windows_mpd::queue_v3::Source::Plex{server_id:"server-one".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:"42".into()}};assert!(media.matches(&binding));binding.index=2;assert!(!media.matches(&binding));binding.index=3;if let crate::windows_mpd::queue_v3::Source::Plex{server_id,..}=&mut binding.source{*server_id="other".into();}assert!(!media.matches(&binding));
  for raw in [serde_json::json!({"kind":"navidrome","connection_id":"x","song_id":"y","queue_index":0}),serde_json::json!({"kind":"plex","server_id":"one","part_key":"/part","queue_index":1000}),serde_json::json!({"kind":"plex","server_id":"one","part_key":"/part","queue_index":-1})]{assert!(RemoteMedia::decode(&raw).is_none());}
 }
 #[test]
 fn optional_remote_read_permit_stays_bounded_until_its_owner_is_dropped(){let first=RemotePermit::acquire().unwrap();assert!(RemotePermit::acquire().is_none());drop(first);assert!(RemotePermit::acquire().is_some());}
}
