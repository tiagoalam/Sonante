//! Windows storage boundary for the unchanged scrobbling account/ledger contract.
use super::{http,model::{Pending,Provider,Target}};
use serde::{Serialize,Deserialize};
use std::{path::Path,io::Read,time::Instant};
use crate::windows_credentials::{self as secret,Purpose,Sensitive};
use base64::Engine;
const STORAGE:&str="SCROBBLING_STORAGE_UNAVAILABLE";
const MAX_PENDING:usize=500;
const MAX_BYTES:u64=4*1024*1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lastfm {
    pub(super) enabled: bool,
    pub(super) username: String,
    pub(super) api_key: String,
    pub(super) api_secret: String,
    pub(super) session: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Listenbrainz {
    pub(super) enabled: bool,
    pub(super) username: String,
    pub(super) token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    pub(super) version: u32,
    pub(super) next_id: u64,
    pub(super) lastfm: Option<Lastfm>,
    pub(super) listenbrainz: Option<Listenbrainz>,
    pub(super) pending: Vec<Pending>,
}
impl Default for File {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            lastfm: None,
            listenbrainz: None,
            pending: vec![],
        }
    }
}
impl File {
    pub(super) fn account(&self, p: Provider) -> Option<&str> {
        match p {
            Provider::Lastfm => self.lastfm.as_ref().map(|v| v.username.as_str()),
            Provider::Listenbrainz => self.listenbrainz.as_ref().map(|v| v.username.as_str()),
        }
    }
    pub(super) fn enabled(&self, p: Provider) -> bool {
        match p {
            Provider::Lastfm => self.lastfm.as_ref().is_some_and(|v| v.enabled),
            Provider::Listenbrainz => self.listenbrainz.as_ref().is_some_and(|v| v.enabled),
        }
    }
    pub(super) fn targets(&self, since: &[Instant; 2]) -> [Option<Target>; 2] {
        Provider::all().map(|p| {
            self.account(p).filter(|_| self.enabled(p)).map(|a| Target {
                account: a.to_string(),
                since: since[p.index()],
            })
        })
    }
    pub(super) fn validate(&self) -> bool {
        if self.version != 1 || self.next_id == 0 || self.pending.len() > MAX_PENDING {
            return false;
        }
        if self.lastfm.as_ref().is_some_and(|v| {
            !http::user(&v.username)
                || !http::hex_key(&v.api_key)
                || !http::hex_key(&v.api_secret)
                || !http::lastfm_credential(&v.session)
        }) {
            return false;
        }
        if self
            .listenbrainz
            .as_ref()
            .is_some_and(|v| !http::user(&v.username) || !http::lb_token(&v.token))
        {
            return false;
        }
        let mut ids = std::collections::HashSet::new();
        self.pending.iter().all(|p| {
            p.id > 0
                && p.id < self.next_id
                && ids.insert(p.id)
                && p.track.valid()
                && p.started_at > 0
                && p.started_at <= 253_402_300_799
                && self.account(p.provider) == Some(p.account.as_str())
        })
    }
}

// Secrets fit the existing bounded DPAPI frame; the independent pending ledger
// retains the original 500-entry/4MiB bound. No plaintext credential fallback.
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Secrets { lastfm:Option<Lastfm>, listenbrainz:Option<Listenbrainz> }
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Record { version:u32,next_id:u64,credentials:String,pending:Vec<Pending> }
type Open=fn(Purpose,&[u8])->secret::Result<Sensitive>;
type Seal=fn(Purpose,&[u8])->secret::Result<Vec<u8>>;
fn read(path:&Path,open:Open)->Result<File,String>{
 #[cfg(windows)] super::private_file::local_path(path)?;
 let mut options=std::fs::OpenOptions::new();options.read(true);
 #[cfg(windows)] {
  use std::os::windows::fs::OpenOptionsExt;
  use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT,FILE_FLAG_OPEN_NO_RECALL,FILE_SHARE_READ};
  options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_OPEN_NO_RECALL).share_mode(FILE_SHARE_READ);
 }
 #[cfg(unix)] {
  use std::os::unix::fs::OpenOptionsExt;
  options.custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK);
 }
 let file=match options.open(path){
  Ok(f)=>f,
  Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return match std::fs::symlink_metadata(path){Err(e)if e.kind()==std::io::ErrorKind::NotFound=>Ok(File::default()),_=>Err(STORAGE.into())},
  Err(_)=>return Err(STORAGE.into()),
 };
 let metadata=file.metadata().map_err(|_|STORAGE)?;
 if !metadata.is_file()||crate::persistence::linked(&metadata)||metadata.len()>MAX_BYTES{return Err(STORAGE.into());}
 #[cfg(windows)] {
  use std::os::windows::fs::MetadataExt;
  use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_OFFLINE,FILE_ATTRIBUTE_RECALL_ON_OPEN,FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS};
  if metadata.file_attributes()&(FILE_ATTRIBUTE_OFFLINE|FILE_ATTRIBUTE_RECALL_ON_OPEN|FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)!=0{return Err(STORAGE.into());}
 }
 #[cfg(windows)] super::private_file::verify(&file)?;
 let mut bytes=Vec::new();file.take(MAX_BYTES+1).read_to_end(&mut bytes).map_err(|_|STORAGE)?;
 if bytes.len()as u64>MAX_BYTES{return Err(STORAGE.into());}
 let record:Record=serde_json::from_slice(&bytes).map_err(|_|STORAGE)?;
 if record.version!=1||record.pending.len()>MAX_PENDING||record.credentials.is_empty()||record.credentials.len()>secret::MAX_SEALED.div_ceil(3)*4{return Err(STORAGE.into());}
 let ciphertext=base64::engine::general_purpose::STANDARD.decode(&record.credentials).map_err(|_|STORAGE)?;
 let plain=open(Purpose::Scrobbling,&ciphertext).map_err(|_|STORAGE)?;
 let secrets:Secrets=serde_json::from_slice(plain.bytes()).map_err(|_|STORAGE)?;
 let data=File{version:record.version,next_id:record.next_id,lastfm:secrets.lastfm,listenbrainz:secrets.listenbrainz,pending:record.pending};
 if !data.validate(){return Err(STORAGE.into());}Ok(data)
}
pub(super) fn load(path:&Path)->Result<File,String>{read(path,secret::open)}
fn write(path:&Path,data:&File,seal:Seal,open:Open)->Result<(),String>{
 // Unknown/corrupt existing bytes are never overwritten by connect/disconnect,
 // a background delivery or an empty/default record.
 drop(read(path,open)?);
 if !data.validate(){return Err(STORAGE.into());}
 let mut plain=Sensitive::new(Vec::with_capacity(secret::MAX_PAYLOAD+1));
 serde_json::to_writer(plain.writer(),&Secrets{lastfm:data.lastfm.clone(),listenbrainz:data.listenbrainz.clone()}).map_err(|_|STORAGE)?;
 let protected=seal(Purpose::Scrobbling,plain.bytes()).map_err(|_|STORAGE)?;
 let bytes=serde_json::to_vec(&Record{version:1,next_id:data.next_id,credentials:base64::engine::general_purpose::STANDARD.encode(protected),pending:data.pending.clone()}).map_err(|_|STORAGE)?;
 if bytes.len()as u64>MAX_BYTES{return Err(STORAGE.into());}
 #[cfg(windows)] {super::private_file::atomic(path,&bytes)}
 #[cfg(not(windows))] {crate::persistence::atomic_write_private(path,&bytes,"windows-scrobbling-v1.json").map_err(|_|STORAGE.into())}
}
pub(super) fn save(path:&Path,data:&File)->Result<(),String>{write(path,data,secret::seal,secret::open)}

#[cfg(test)]mod tests{
 use super::*;
 struct Fixture(std::path::PathBuf);
 impl Fixture{fn new()->Self{let mut n=[0;12];getrandom::fill(&mut n).unwrap();let root=std::env::temp_dir().join(format!("sonante-scrobble-store-{n:x?}"));std::fs::create_dir(&root).unwrap();Self(root)}fn path(&self)->std::path::PathBuf{self.0.join("own-record.json")}}
 impl Drop for Fixture{fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}
 fn data()->File{
  let mut d=File::default();d.lastfm=Some(Lastfm{enabled:true,username:"fixture-a".into(),api_key:"a".repeat(32),api_secret:"b".repeat(32),session:"G+/&?#=%".repeat(4)});
  d.listenbrainz=Some(Listenbrainz{enabled:true,username:"fixture-b".into(),token:"SYNTHETIC-FIXTURE-TOKEN".into()});
  d.pending.push(Pending{id:1,provider:Provider::Lastfm,account:"fixture-a".into(),track:super::super::model::Track::new("Artist","Track","Album",120.).unwrap(),started_at:100,next_attempt:160,attempts:1});d.next_id=2;d
 }
 fn write_fixture(path:&Path,data:&File)->Result<(),String>{write(path,data,secret::fixture_seal,secret::fixture_open)}
 fn read_fixture(path:&Path)->Result<File,String>{read(path,secret::fixture_open)}
 #[test]fn credential_envelope_and_pending_ledger_roundtrip_without_plaintext_credential_fields(){
  let f=Fixture::new();let path=f.path();assert!(read_fixture(&path).unwrap().validate());let d=data();write_fixture(&path,&d).unwrap();let raw=std::fs::read(&path).unwrap();let json:serde_json::Value=serde_json::from_slice(&raw).unwrap();
  assert_eq!(json["version"],1);for forbidden in ["api_key","api_secret","session","token","lastfm","listenbrainz"]{assert!(json.get(forbidden).is_none());}
  // Fixture codec is deliberately NOT encryption. Native DPAPI is a separate gate.
  let restored=read_fixture(&path).unwrap();assert!(restored.validate());assert_eq!(restored.lastfm.unwrap().session,"G+/&?#=%".repeat(4));assert_eq!(restored.listenbrainz.unwrap().token,"SYNTHETIC-FIXTURE-TOKEN");assert_eq!(restored.pending[0].next_attempt,160);assert_eq!(restored.pending[0].attempts,1);
 }
 #[test]fn corrupt_record_is_never_reset_by_a_background_write_or_disconnect(){
  let f=Fixture::new();let path=f.path();for bytes in [b"".as_slice(),b"invalid",br#"{"version":9}"#,br#"{"version":1,"next_id":1,"credentials":"invalid","pending":[]}"#]{std::fs::write(&path,bytes).unwrap();assert!(read_fixture(&path).is_err());assert!(write_fixture(&path,&File::default()).is_err());assert_eq!(std::fs::read(&path).unwrap(),bytes);}
 }
 #[test]fn foreign_purpose_wrong_account_duplicate_id_and_budgets_are_rejected(){
  let f=Fixture::new();let path=f.path();let mut d=data();d.pending[0].account="other".into();assert!(write_fixture(&path,&d).is_err());d=data();d.pending.push(d.pending[0].clone());assert!(write_fixture(&path,&d).is_err());
  d=data();d.next_id=0;assert!(write_fixture(&path,&d).is_err());d=data();d.listenbrainz.as_mut().unwrap().token="bad token".into();assert!(write_fixture(&path,&d).is_err());
  let foreign=secret::fixture_seal(Purpose::Plex,br#"{"lastfm":null,"listenbrainz":null}"#).unwrap();let record=Record{version:1,next_id:1,credentials:base64::engine::general_purpose::STANDARD.encode(foreign),pending:vec![]};std::fs::write(&path,serde_json::to_vec(&record).unwrap()).unwrap();assert!(read_fixture(&path).is_err());let bytes=std::fs::read(&path).unwrap();assert!(write_fixture(&path,&File::default()).is_err());assert_eq!(std::fs::read(&path).unwrap(),bytes);
 }
 #[test]fn disabled_and_disconnected_accounts_do_not_restore_pending_cross_account_jobs(){
  let f=Fixture::new();let path=f.path();let mut d=data();d.lastfm.as_mut().unwrap().enabled=false;d.pending.retain(|p|p.provider!=Provider::Lastfm);write_fixture(&path,&d).unwrap();let mut d=read_fixture(&path).unwrap();assert!(!d.enabled(Provider::Lastfm));assert!(d.pending.is_empty());assert!(d.enabled(Provider::Listenbrainz));d.lastfm=None;write_fixture(&path,&d).unwrap();let d=read_fixture(&path).unwrap();assert!(d.account(Provider::Lastfm).is_none());assert_eq!(d.account(Provider::Listenbrainz),Some("fixture-b"));
 }
 #[cfg(unix)]#[test]fn dangling_symlink_and_large_records_are_not_absent_or_repaired(){
  let f=Fixture::new();let path=f.path();std::os::unix::fs::symlink(f.0.join("missing"),&path).unwrap();assert!(read_fixture(&path).is_err());assert!(write_fixture(&path,&File::default()).is_err());assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());std::fs::remove_file(&path).unwrap();let bytes=vec![b'x';MAX_BYTES as usize+1];std::fs::write(&path,&bytes).unwrap();assert!(read_fixture(&path).is_err());assert!(write_fixture(&path,&File::default()).is_err());assert_eq!(std::fs::read(&path).unwrap(),bytes);
 }
}
