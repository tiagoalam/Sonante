//! FF06-I1a disconnected, preliminary filesystem inspection only.
//! This module does NOT admit product reads/playback. No Lease, transaction,
//! recovery, config-generation, or UI/MPD bridge exists here. The caller-supplied
//! config slice is a consistency input, never evidence of a live product gate.
//! Paths are preliminary and retain a validation/open TOCTOU window.
use std::{cell::Cell, fs, sync::{Arc,Mutex}, path::{Path,PathBuf}};
use serde::Serialize;
use crate::{local_media::{self,mirror::{ReadonlyMirrorSnapshot,ReadonlyMirrorOpenError}},
    source_identity::{ReadonlyIdentitySnapshot,ReadonlyOriginObservation}};

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum InspectionError {
    InventoryUnconfirmed, ConfigBindingUnconfirmed, SnapshotChanged,
    #[cfg(test)] ObservedIdentityConflict,
    #[cfg(test)] InvalidUri,
    UnknownSource,
    #[cfg(test)] TargetMissing,
    #[cfg(test)] OutsideRoot,
    #[cfg(test)] SourceUnavailable,
}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum AvailabilityStatus { Available, Unavailable, IdentityConflict }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum AvailabilityReason { NotFound, PermissionDenied, IoError, OriginMismatch }
/// Observations only. No roots, Origin, raw error, action booleans, or token.
#[derive(Clone,Debug,PartialEq,Eq,Serialize)]
pub(crate) struct PreliminarySourceRow {
    pub source_id:String,
    // Label is the already validated stable ID, not arbitrary filesystem text.
    pub label:String,
    pub status:AvailabilityStatus,
    pub reason:Option<AvailabilityReason>,
}
#[derive(Serialize)]
pub(crate) struct PreliminaryObservation {
    pub admission:PreliminaryOnly,
    pub sources:Vec<PreliminarySourceRow>,
    pub observed_conflict:bool,
}
#[derive(Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum PreliminaryOnly { NotProductAdmission }
/// Private fields prevent fabrication of a purported trusted inventory.
/// There is intentionally no Serialize/Debug or public constructor accepting a gate bool.
pub(crate) struct PreliminaryInventory {
    identity:ReadonlyIdentitySnapshot,
    mirror:Arc<ReadonlyMirrorSnapshot>,
    configured:Vec<String>,
    labels:std::collections::BTreeMap<String,String>,
    library:PathBuf,
    observed_conflict:Cell<bool>,
}
/// NOT a product-readable capability. No consumer is connected in this lot.
/// Residual path race and missing product gates forbid treating it as authorization.
#[cfg(test)]
pub(crate) struct PreliminaryTarget { path:PathBuf, source_id:String }
#[cfg(test)]
impl PreliminaryTarget {
    pub(crate) fn preliminary_path(&self)->&Path { &self.path }
    pub(crate) fn source_id(&self)->&str { &self.source_id }
}
// Only the mirror's immutable topology proof is shared. Config, identity,
// physical origins, Lease and transaction admission remain fresh per request.
#[derive(Default)]
pub(crate) struct MirrorReadCache(Mutex<Option<(PathBuf,Arc<ReadonlyMirrorSnapshot>)>>);
impl MirrorReadCache {
    fn get(&self,library:&Path)->Result<Arc<ReadonlyMirrorSnapshot>,ReadonlyMirrorOpenError> {
        let mut slot=self.0.lock().map_err(|_|ReadonlyMirrorOpenError::Unconfirmed)?;
        if let Some((path,snapshot))=slot.as_ref() {
            if path==library && snapshot.revalidate_binding().is_ok() {
                if snapshot.revalidate().is_err() {
                    *slot=None;return Err(ReadonlyMirrorOpenError::Unconfirmed);
                }
                return Ok(Arc::clone(snapshot));
            }
            // A new request may capture a newly published generation, before
            // reading any target. An in-flight request never switches proofs.
            *slot=None;
        }
        let snapshot=Arc::new(ReadonlyMirrorSnapshot::open_cached(library)?);
        *slot=Some((library.into(),Arc::clone(&snapshot)));Ok(snapshot)
    }
}
pub(crate) fn with_cached_read_inventory<T>(
    cache:&MirrorReadCache,library:&Path,configured:&[String],
    read:impl FnOnce(&PreliminaryObservation,
        &dyn Fn()->Result<PreliminaryObservation,String>,&PreliminaryInventory)->Result<T,String>,
)->Result<(T,PreliminaryObservation),String> {
    let mirror=cache.get(library).map_err(|error|match error {
        ReadonlyMirrorOpenError::UnsupportedSchema=>"SOURCE_AVAILABILITY_UNSUPPORTED_FORMAT",
        ReadonlyMirrorOpenError::Unconfirmed=>"SOURCE_AVAILABILITY_CHANGED",
    })?;
    let inventory=PreliminaryInventory::capture_with_mirror(library,configured,mirror)
        .map_err(|_|"SOURCE_AVAILABILITY_UNCONFIRMED")?;
    finish_read_inventory(inventory,configured,read)
}
// The owner of the real config/startup/Lease/Coordinator guards calls this
// finite read scope. The initial mirror is fully validated by open(); every
// original entry is checked again before any callback value can escape.
// Intermediate observations recheck bound identity/manifest and real D03 roots,
// rather than walking unrelated entries repeatedly within the same request.
#[cfg(test)]
pub(crate) fn with_read_inventory<T>(
    library:&Path,configured:&[String],
    read:impl FnOnce(&PreliminaryObservation,
        &dyn Fn()->Result<PreliminaryObservation,String>,&PreliminaryInventory)->Result<T,String>,
)->Result<(T,PreliminaryObservation),String> {
    let inventory=PreliminaryInventory::capture(library,configured)
        .map_err(|_|"SOURCE_AVAILABILITY_UNCONFIRMED")?;
    finish_read_inventory(inventory,configured,read)
}
fn finish_read_inventory<T>(
    inventory:PreliminaryInventory,configured:&[String],
    read:impl FnOnce(&PreliminaryObservation,
        &dyn Fn()->Result<PreliminaryObservation,String>,&PreliminaryInventory)->Result<T,String>,
)->Result<(T,PreliminaryObservation),String> {
    let before=inventory.observe_read_origins(configured)
        .map_err(|_|"SOURCE_AVAILABILITY_UNCONFIRMED")?;
    let recheck=||inventory.observe_read_origins(configured)
        .map_err(|_|"SOURCE_AVAILABILITY_UNCONFIRMED".to_string());
    let value=read(&before,&recheck,&inventory)?;
    inventory.revalidate(configured).map_err(|_|"SOURCE_AVAILABILITY_CHANGED")?;
    Ok((value,before))
}

impl PreliminaryInventory {
    #[cfg(test)]
    pub(crate) fn inspect(library:&Path,configured:&[String])->Result<Self,InspectionError> {
        let s=Self::capture(library,configured)?;
        s.revalidate(configured)?; Ok(s)
    }
    #[cfg(test)]
    fn capture(library:&Path,configured:&[String])->Result<Self,InspectionError> {
        let mirror=Arc::new(ReadonlyMirrorSnapshot::open(library).map_err(|_|InspectionError::InventoryUnconfirmed)?);
        Self::capture_with_mirror(library,configured,mirror)
    }
    fn capture_with_mirror(library:&Path,configured:&[String],mirror:Arc<ReadonlyMirrorSnapshot>)->Result<Self,InspectionError> {
        let identity=ReadonlyIdentitySnapshot::open(library).map_err(|_|InspectionError::InventoryUnconfirmed)?;
        if !identity.matches_manifest(mirror.manifest.registry_id.as_deref().ok_or(InspectionError::InventoryUnconfirmed)?,&mirror.manifest.sources) {
            return Err(InspectionError::InventoryUnconfirmed);
        }
        Self::check_binding(&mirror,configured)?;
        let labels=local_media::source_display_labels(&mirror.manifest.sources);
        let s=Self {identity,mirror,configured:configured.to_vec(),labels,library:library.into(),observed_conflict:Cell::new(false)};
        s.revalidate_read_bindings(configured)?; Ok(s)
    }
    #[cfg(test)]
    pub(crate) fn read_cover_pair<T>(
        &self, owner:&crate::profile_lease::Lease,
        transaction:&crate::config_save_operation::Transaction<'_>,
        coordinator:&crate::config_save_operation::Coordinator,
        library:&Path,configured:&[String],paths:&[String],
        read:impl FnMut(&str,&local_media::SourceCoverTarget<'_>)->Result<T,String>,
    )->Result<Vec<Result<T,String>>,String> {
        self.revalidate(configured).map_err(|_|"LOCAL_COVER_CONTEXT_CHANGED")?;
        let values=self.read_cover_pair_in_context(owner,transaction,coordinator,library,configured,paths,read)?;
        self.revalidate(configured).map_err(|_|"LOCAL_COVER_CONTEXT_CHANGED")?;
        Ok(values)
    }
    // Called only inside with_read_inventory, whose final full validation must
    // succeed before this data is published. Standalone consumers use the
    // full-fenced read_cover_pair above.
    pub(crate) fn read_cover_pair_in_context<T>(
        &self, owner:&crate::profile_lease::Lease,
        transaction:&crate::config_save_operation::Transaction<'_>,
        coordinator:&crate::config_save_operation::Coordinator,
        library:&Path,configured:&[String],paths:&[String],
        mut read:impl FnMut(&str,&local_media::SourceCoverTarget<'_>)->Result<T,String>,
    )->Result<Vec<Result<T,String>>,String> {
        if paths.is_empty() || paths.len()>2 {return Err("LOCAL_COVER_BATCH_BOUND".into());}
        if library!=self.library.as_path() {return Err("LOCAL_COVER_CONTEXT_CHANGED".into());}
        self.revalidate_read_bindings(configured).map_err(|_|"LOCAL_COVER_CONTEXT_CHANGED")?;
        let values=paths.iter().map(|path|local_media::with_source_cover_target_in_snapshot(
            owner,transaction,coordinator,library,configured,path,&self.identity,&self.mirror,
            |target|read(path,target))).collect();
        self.revalidate_read_bindings(configured).map_err(|_|"LOCAL_COVER_CONTEXT_CHANGED")?;
        Ok(values)
    }
    fn revalidate_read_bindings(&self,configured:&[String])->Result<(),InspectionError> {
        if configured!=self.configured {return Err(InspectionError::SnapshotChanged);}
        Self::check_binding(&self.mirror,configured)?;
        self.identity.revalidate().map_err(|_|InspectionError::SnapshotChanged)?;
        self.mirror.revalidate_binding().map_err(|_|InspectionError::SnapshotChanged)
    }
    fn observe_read_origins(&self,configured:&[String])->Result<PreliminaryObservation,InspectionError> {
        self.revalidate_read_bindings(configured)?;
        let sources=self.mirror.manifest.sources.keys().map(|id|self.observe_one(id)).collect::<Result<Vec<_>,_>>()?;
        self.revalidate_read_bindings(configured)?;
        Ok(PreliminaryObservation {admission:PreliminaryOnly::NotProductAdmission,sources,observed_conflict:self.observed_conflict.get()})
    }
    fn check_binding(mirror:&ReadonlyMirrorSnapshot,configured:&[String])->Result<(),InspectionError> {
        if !crate::local_media::configured_source_binding_matches(configured,&mirror.manifest.sources) {
            return Err(InspectionError::ConfigBindingUnconfirmed);
        }
        Ok(())
    }
    fn revalidate(&self,configured:&[String])->Result<(),InspectionError> {
        if configured!=self.configured { return Err(InspectionError::SnapshotChanged); }
        Self::check_binding(&self.mirror,configured)?;
        self.identity.revalidate().map_err(|_|InspectionError::SnapshotChanged)?;
        self.mirror.revalidate().map_err(|_|InspectionError::SnapshotChanged)
    }
    fn observe_one(&self,id:&str)->Result<PreliminarySourceRow,InspectionError> {
        let root=self.mirror.manifest.sources.get(id).ok_or(InspectionError::UnknownSource)?;
        use {AvailabilityStatus::*,AvailabilityReason::*,ReadonlyOriginObservation as O};
        let (status,reason)=match self.identity.observe(id).map_err(|_|InspectionError::SnapshotChanged)? {
            // D03 alone does not reject an ancestor symlink to the same inode.
            // Keep the observation consistent with target probing: inspect only
            // this ID's canonical root spelling before reporting Available.
            O::Available=>match fs::canonicalize(root) {
                Ok(current) if current.as_os_str()==root.as_os_str() => (Available,None),
                Ok(_) => {self.observed_conflict.set(true); (IdentityConflict,Some(OriginMismatch))},
                Err(e) => (Unavailable,Some(match e.kind() {
                    std::io::ErrorKind::NotFound=>NotFound,
                    std::io::ErrorKind::PermissionDenied=>PermissionDenied,
                    _=>IoError,
                })),
            },
            O::NotFound=>(Unavailable,Some(NotFound)),
            O::PermissionDenied=>(Unavailable,Some(PermissionDenied)), O::IoError=>(Unavailable,Some(IoError)),
            O::Conflict=>{self.observed_conflict.set(true); (IdentityConflict,Some(OriginMismatch))},
        };
        Ok(PreliminarySourceRow {source_id:id.into(),label:self.labels.get(id).cloned().ok_or(InspectionError::UnknownSource)?,status,reason})
    }
    /// Explicit full observation may perform IO on A. probe_target(B) does not call it.
    #[cfg(test)]
    pub(crate) fn observe_all(&self,configured:&[String])->Result<PreliminaryObservation,InspectionError> {
        self.revalidate(configured)?;
        let sources=self.mirror.manifest.sources.keys().map(|id|self.observe_one(id)).collect::<Result<Vec<_>,_>>()?;
        self.revalidate(configured)?;
        Ok(PreliminaryObservation {admission:PreliminaryOnly::NotProductAdmission,sources,observed_conflict:self.observed_conflict.get()})
    }
    #[cfg(test)]
    pub(crate) fn probe_target(&self,configured:&[String],uri:&str)->Result<PreliminaryTarget,InspectionError> {
        self.revalidate(configured)?;
        if self.observed_conflict.get() { return Err(InspectionError::ObservedIdentityConflict); }
        // Forbid normalization aliases in a request, in addition to existing URI validation.
        if uri.len()>4096 || !local_media::is_safe_relative_mpd_uri(uri)
            || uri.split('/').any(|s|s.is_empty() || s=="." || s=="..") {
            return Err(InspectionError::InvalidUri);
        }
        let mut parts=uri.split('/'); let id=parts.next().ok_or(InspectionError::InvalidUri)?;
        let root=self.mirror.manifest.sources.get(id).ok_or(InspectionError::UnknownSource)?;
        let row=self.observe_one(id)?;
        if row.status==AvailabilityStatus::IdentityConflict { return Err(InspectionError::ObservedIdentityConflict); }
        if row.status!=AvailabilityStatus::Available { return Err(InspectionError::SourceUnavailable); }
        // All remaining components were checked; no absolute path or traversal.
        let candidate=parts.fold(root.clone(),|p,c|p.join(c));
        let canonical=fs::canonicalize(&candidate).map_err(|e|match e.kind() {
            std::io::ErrorKind::NotFound=>InspectionError::TargetMissing, _=>InspectionError::SourceUnavailable })?;
        if !canonical.starts_with(root) { return Err(InspectionError::OutsideRoot); }
        // A root replaced through an ancestor symlink may reach the same inode;
        // require actual canonical spelling as well as D03, without probing A.
        let current_root=fs::canonicalize(root).map_err(|_|InspectionError::SourceUnavailable)?;
        if current_root.as_os_str()!=root.as_os_str() { self.observed_conflict.set(true); return Err(InspectionError::ObservedIdentityConflict); }
        let m=fs::metadata(&canonical).map_err(|_|InspectionError::SourceUnavailable)?;
        if !m.is_file() && !m.is_dir() { return Err(InspectionError::InvalidUri); }
        let after=self.observe_one(id)?;
        if after.status==AvailabilityStatus::IdentityConflict { return Err(InspectionError::ObservedIdentityConflict); }
        if after.status!=AvailabilityStatus::Available { return Err(InspectionError::SourceUnavailable); }
        self.revalidate(configured)?;
        Ok(PreliminaryTarget {path:canonical,source_id:id.into()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap,os::{fd::{AsRawFd,FromRawFd},unix::{ffi::OsStrExt,fs::{MetadataExt,PermissionsExt,symlink}}},
        sync::atomic::{AtomicU64,Ordering}};
    static NEXT:AtomicU64=AtomicU64::new(0);
    // Retain ALL fixtures, including successful ones, until a separately admitted
    // parent has collected completion + snapshots. No Drop may delete a scene.
    // Descriptor/name ownership below is for setup/collection consistency, not a
    // claim of protection from arbitrary same-UID hostile concurrent filesystem.
    struct OwnedFixtureRoot {
        parent:std::fs::File, parent_path:PathBuf,
        directory:std::fs::File, name:std::ffi::CString, path:PathBuf,
    }
    impl OwnedFixtureRoot {
        fn same_owned_directory(file:&std::fs::File,path:&Path) {
            let opened=file.metadata().unwrap(); let named=fs::symlink_metadata(path).unwrap();
            assert!(opened.is_dir() && named.is_dir());
            assert_eq!(opened.uid(),unsafe{libc::geteuid()});
            assert_eq!(opened.mode() & 0o7777,0o700);
            assert_eq!((opened.dev(),opened.ino()),(named.dev(),named.ino()));
            assert_eq!((opened.uid(),opened.mode()),(named.uid(),named.mode()));
        }
        fn validate(&self) {
            Self::same_owned_directory(&self.parent,&self.parent_path);
            assert_eq!(fs::canonicalize(&self.parent_path).unwrap().as_os_str(),self.parent_path.as_os_str());
            let anchored=PathBuf::from(format!("/proc/self/fd/{}",self.parent.as_raw_fd()))
                .join(std::ffi::OsStr::from_bytes(self.name.as_bytes()));
            Self::same_owned_directory(&self.directory,&anchored);
            Self::same_owned_directory(&self.directory,&self.path);
            assert_eq!(fs::canonicalize(&self.path).unwrap().as_os_str(),self.path.as_os_str());
        }
        fn create()->Self {
            use std::os::unix::fs::OpenOptionsExt;
            let parent_path=PathBuf::from(std::env::var_os("FF06_I1A_OWN_FIXTURES").expect("future own-FS harness required"));
            assert!(parent_path.is_absolute());
            assert_eq!(fs::canonicalize(&parent_path).unwrap().as_os_str(),parent_path.as_os_str());
            let parent=fs::OpenOptions::new().read(true)
                .custom_flags(libc::O_NOFOLLOW|libc::O_DIRECTORY|libc::O_CLOEXEC).open(&parent_path).unwrap();
            Self::same_owned_directory(&parent,&parent_path);
            let child=format!("case-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::SeqCst));
            let name=std::ffi::CString::new(child.clone()).unwrap();
            // mkdirat fails on any existing entry: ownership never comes from a
            // prefix, PID, or counter. Failed partial setup is retained too.
            assert_eq!(unsafe{libc::mkdirat(parent.as_raw_fd(),name.as_ptr(),0o700)},0);
            let fd=unsafe{libc::openat(parent.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
            assert!(fd>=0);
            let directory=unsafe{std::fs::File::from_raw_fd(fd)};
            let path=parent_path.join(child);
            let owned=Self{parent,parent_path,directory,name,path};owned.validate();owned
        }
    }
    struct Fixture { root:PathBuf, profile:PathBuf, library:PathBuf, a:PathBuf, b:PathBuf,
        config:Vec<String>, _lease:crate::profile_lease::Lease, owned:OwnedFixtureRoot }
    impl Fixture {
        fn new(empty_a:bool)->Self { Self::new_with_extra_b(empty_a,0) }
        fn new_with_extra_b(empty_a:bool, extra_b:usize)->Self {
            // Runtime is intentionally not authorized in I1a. A future harness
            // must explicitly provide a dedicated, private own-fixture parent.
            let owned=OwnedFixtureRoot::create();let root=owned.path.clone();
            let profile=root.join("profile");fs::create_dir(&profile).unwrap();
            fs::set_permissions(&profile,fs::Permissions::from_mode(0o700)).unwrap();
            let lease=crate::profile_lease::Lease::acquire(&profile).unwrap();
            let roots=root.join("roots");fs::create_dir(&roots).unwrap();
            let a=roots.join("A");let b=roots.join("B");fs::create_dir(&a).unwrap();fs::create_dir(&b).unwrap();
            if !empty_a {fs::write(a.join("a.flac"),b"own fixture A metadata only").unwrap();}
            fs::write(b.join("b.flac"),b"own fixture B metadata only").unwrap();
            for i in 0..extra_b {fs::write(b.join(format!("scale-{i}.flac")),b"own metadata").unwrap();}
            let library=profile.join("library");let staging=profile.join("staging");
            // Legitimate product initialization. No positive identity/checksum is forged.
            crate::local_media::mirror::build_filtered_staging(&staging,&library,&[a.clone(),b.clone()],"ff06-i1a-fixture").unwrap();
            fs::rename(&staging,&library).unwrap();
            let config=vec![a.to_str().unwrap().into(),b.to_str().unwrap().into()];
            owned.validate();
            Self {root,profile,library,a,b,config,_lease:lease,owned}
        }
        fn inspect(&self)->Result<PreliminaryInventory,InspectionError> { PreliminaryInventory::inspect(&self.library,&self.config) }
        fn id(&self,p:&Path)->String {
            crate::local_media::mirror::read_manifest(&self.library).unwrap().sources.iter()
                .find(|(_,r)|r.as_path()==p).unwrap().0.clone()
        }
        fn inventory(&self)->BTreeMap<PathBuf,(u64,u64,u32,u64,Vec<u8>)> {
            fn walk(root:&Path,p:&Path,out:&mut BTreeMap<PathBuf,(u64,u64,u32,u64,Vec<u8>)>) {
                use std::os::unix::ffi::OsStrExt;
                let m=fs::symlink_metadata(p).unwrap();
                let bytes=if m.file_type().is_symlink(){fs::read_link(p).unwrap().as_os_str().as_bytes().to_vec()}
                    else if m.is_file(){fs::read(p).unwrap()}else{Vec::new()};
                out.insert(p.strip_prefix(root).unwrap().into(),(m.dev(),m.ino(),m.mode(),m.nlink(),bytes));
                if m.is_dir(){for e in fs::read_dir(p).unwrap(){walk(root,&e.unwrap().path(),out);}}
            }
            self.owned.validate();
            let mut out=BTreeMap::new();walk(&self.root,&self.root,&mut out);
            self.owned.validate();out
        }
        fn readonly<T>(&self,f:impl FnOnce()->T)->T {
            let before=self.inventory();let value=f();assert_eq!(before,self.inventory());value
        }
    }
    // No Drop implementation: successful, failed, panicked and partial setup
    // scenes all remain available to the future parent collector.
    #[test] fn offline_a_keeps_complete_rows_and_b_readback() {
        let f=Fixture::new(false);let a=f.id(&f.a);let b=f.id(&f.b);
        fs::rename(&f.a,f.root.join("A-away")).unwrap();
        f.readonly(||{
            let i=f.inspect().unwrap();let target=i.probe_target(&f.config,&format!("{b}/b.flac")).unwrap();
            assert_eq!(target.source_id(),b);assert_eq!(fs::read(target.preliminary_path()).unwrap(),b"own fixture B metadata only");
            let rows=i.observe_all(&f.config).unwrap();assert_eq!(rows.sources.len(),2);
            assert_eq!(rows.sources.iter().find(|r|r.source_id==a).unwrap().reason,Some(AvailabilityReason::NotFound));
        });
    }
    #[test] fn return_a_same_id_and_empty_is_available() {
        let f=Fixture::new(true);let a=f.id(&f.a);let i=f.inspect().unwrap();
        fs::rename(&f.a,f.root.join("A-away")).unwrap();
        assert_eq!(i.observe_all(&f.config).unwrap().sources.iter().find(|r|r.source_id==a).unwrap().status,AvailabilityStatus::Unavailable);
        fs::rename(f.root.join("A-away"),&f.a).unwrap();
        f.readonly(||{
            let row=i.observe_all(&f.config).unwrap().sources.into_iter().find(|r|r.source_id==a).unwrap();
            assert_eq!(row.status,AvailabilityStatus::Available);assert_eq!(row.reason,None);
        });
    }
    #[test] fn replaced_b_conflicts_and_blocks_subsequent_probes() {
        let f=Fixture::new(false);let b=f.id(&f.b);let a=f.id(&f.a);let i=f.inspect().unwrap();
        fs::rename(&f.b,f.root.join("B-away")).unwrap();fs::create_dir(&f.b).unwrap();
        f.readonly(||{
            assert!(matches!(i.probe_target(&f.config,&b),Err(InspectionError::ObservedIdentityConflict)));
            assert!(matches!(i.probe_target(&f.config,&a),Err(InspectionError::ObservedIdentityConflict)));
        });
    }
    #[test] fn unknown_traversal_and_escape_refuse() {
        let f=Fixture::new(false);let b=f.id(&f.b);fs::write(f.root.join("outside.flac"),b"not B").unwrap();
        symlink(f.root.join("outside.flac"),f.b.join("escape.flac")).unwrap();
        f.readonly(||{let i=f.inspect().unwrap();
            assert!(matches!(i.probe_target(&f.config,"unknown/x"),Err(InspectionError::UnknownSource)));
            for uri in [format!("{b}/../x"),format!("{b}//x"),format!("{b}/./x"),"https://x".into()] {
                assert!(matches!(i.probe_target(&f.config,&uri),Err(InspectionError::InvalidUri)));
            }
            assert!(matches!(i.probe_target(&f.config,&format!("{b}/escape.flac")),Err(InspectionError::OutsideRoot)));
        });
    }
    #[test] fn corrupt_files_preserved() {
        for name in ["local-source-identities.json","local-source-identities.lock","library/.sonante-mirror.json"] {
            let f=Fixture::new(false);fs::write(f.profile.join(name),b"invalid").unwrap();
            f.readonly(||assert!(f.inspect().is_err()));
        }
    }
    #[test] fn hardlinked_private_and_overlong_anchor_reject() {
        for name in ["local-source-identities.json","local-source-identities.lock","library/.sonante-mirror.json"] {
            let f=Fixture::new(false);fs::hard_link(f.profile.join(name),f.root.join("hardlink")).unwrap();
            f.readonly(||assert!(f.inspect().is_err()));
        }
        let f=Fixture::new(false);fs::write(f.profile.join("local-source-identities.lock"),[b'a';33]).unwrap();
        f.readonly(||assert!(f.inspect().is_err()));
    }
    #[test] fn unsafe_private_modes_and_symlink_anchor_reject() {
        for name in ["local-source-identities.json","local-source-identities.lock"] {
            let f=Fixture::new(false);fs::set_permissions(f.profile.join(name),fs::Permissions::from_mode(0o640)).unwrap();
            f.readonly(||assert!(f.inspect().is_err()));
        }
        let f=Fixture::new(false);let p=f.profile.join("local-source-identities.lock");
        fs::rename(&p,f.root.join("anchor-away")).unwrap();symlink(f.root.join("anchor-away"),&p).unwrap();
        f.readonly(||assert!(f.inspect().is_err()));
    }
    #[test] fn replaced_snapshot_same_bytes_rejects() {
        let f=Fixture::new(false);let b=f.id(&f.b);let i=f.inspect().unwrap();
        let p=f.profile.join("local-source-identities.json");let bytes=fs::read(&p).unwrap();
        fs::rename(&p,f.root.join("ledger-away")).unwrap();fs::write(&p,bytes).unwrap();
        fs::set_permissions(&p,fs::Permissions::from_mode(0o600)).unwrap();
        f.readonly(||assert!(matches!(i.probe_target(&f.config,&b),Err(InspectionError::SnapshotChanged))));
    }
    #[test] fn xrcd_path_binding_reads_trailing_slash_without_mutation_and_keeps_stale_guard() {
        let f=Fixture::new(false);let b=f.id(&f.b);
        let mut configured=f.config.clone();configured[0].push('/');configured[1].push('/');
        f.readonly(||{
            let i=PreliminaryInventory::inspect(&f.library,&configured).unwrap();
            let target=i.probe_target(&configured,&format!("{b}/b.flac")).unwrap();
            assert_eq!(target.source_id(),b);
            assert_eq!(fs::read(target.preliminary_path()).unwrap(),b"own fixture B metadata only");
            assert_eq!(i.observe_all(&configured).unwrap().sources.len(),2);
            // Equivalent roots do not allow a changed config within one read.
            assert!(matches!(i.probe_target(&f.config,&b),Err(InspectionError::SnapshotChanged)));
            let mut duplicate=configured.clone();duplicate[1]=f.config[0].clone();
            assert!(matches!(PreliminaryInventory::inspect(&f.library,&duplicate),Err(InspectionError::ConfigBindingUnconfirmed)));
        });
    }
    #[test] fn config_mismatch_and_stale_slice_refuse() {
        let f=Fixture::new(false);let i=f.inspect().unwrap();let b=f.id(&f.b);let mut config=f.config.clone();config.pop();
        f.readonly(||{
            assert!(matches!(PreliminaryInventory::inspect(&f.library,&config),Err(InspectionError::ConfigBindingUnconfirmed)));
            assert!(matches!(i.probe_target(&config,&b),Err(InspectionError::SnapshotChanged)));
            let mut duplicate=f.config.clone();duplicate.push(duplicate[0].clone());
            assert!(matches!(PreliminaryInventory::inspect(&f.library,&duplicate),Err(InspectionError::ConfigBindingUnconfirmed)));
        });
    }
    #[test] fn offline_alias_and_legacy_refuse_without_initialization() {
        let f=Fixture::new(false);symlink(&f.a,f.root.join("alias-A")).unwrap();
        fs::rename(&f.a,f.root.join("A-away")).unwrap();let mut alias=f.config.clone();alias[0]=f.root.join("alias-A").to_str().unwrap().into();
        f.readonly(||assert!(matches!(PreliminaryInventory::inspect(&f.library,&alias),Err(InspectionError::ConfigBindingUnconfirmed))));
        fs::remove_file(f.library.join(crate::local_media::mirror::MANIFEST_NAME)).unwrap();
        f.readonly(||assert!(f.inspect().is_err()));
    }
    #[test] fn topology_forged_alias_and_stale_tree_refuse() {
        let f=Fixture::new(false);let b=f.id(&f.b);let i=f.inspect().unwrap();let link=f.library.join(&b).join("b.flac");
        fs::remove_file(&link).unwrap();symlink(f.a.join("a.flac"),&link).unwrap();
        f.readonly(||{assert!(f.inspect().is_err());assert!(matches!(i.probe_target(&f.config,&b),Err(InspectionError::SnapshotChanged)));});
    }
    #[test] fn duplicate_json_and_trailing_bytes_are_rejected() {
        use crate::source_identity::readonly_unique_json as parse;
        for bytes in [br#"{"ids":{"same":1,"same":2}}"#.as_slice(),br#"{"x":1,"x":2}"#,br#"{"x":1} {}"#] {
            assert!(parse::<serde_json::Value>(bytes).is_err());
        }
        assert!(parse::<serde_json::Value>(br#"{"x":[1,null,true,"ok"]}"#).is_ok());
    }
    #[test] fn retired_reservation_does_not_become_active_row() {
        let f=Fixture::new(false);let old_a=f.id(&f.a);
        // Legitimate second generation removes A; reservation remains, no ledger forging.
        let staging=f.profile.join("next");
        crate::local_media::mirror::build_filtered_staging(&staging,&f.library,&[f.b.clone()],"ff06-i1a-next").unwrap();
        fs::rename(&f.library,f.profile.join("previous")).unwrap();fs::rename(&staging,&f.library).unwrap();
        let config=vec![f.b.to_str().unwrap().to_string()];
        f.readonly(||{let i=PreliminaryInventory::inspect(&f.library,&config).unwrap();
            let rows=i.observe_all(&config).unwrap();assert_eq!(rows.sources.len(),1);assert_ne!(rows.sources[0].source_id,old_a);});
    }
    #[test] fn ancestor_symlink_observation_and_target_agree() {
        let f=Fixture::new(false);let b=f.id(&f.b);let a=f.id(&f.a);
        let all_first=f.inspect().unwrap();let target_first=f.inspect().unwrap();
        let before=fs::metadata(&f.b).unwrap();
        let roots=f.root.join("roots");let moved=f.root.join("roots-original");
        fs::rename(&roots,&moved).unwrap();symlink(&moved,&roots).unwrap();
        let after=fs::metadata(&f.b).unwrap();
        assert_eq!((before.dev(),before.ino()),(after.dev(),after.ino()));
        f.readonly(||{
            let rows=all_first.observe_all(&f.config).unwrap();assert!(rows.observed_conflict);
            for id in [&a,&b] {
                let row=rows.sources.iter().find(|r|&r.source_id==id).unwrap();
                assert_eq!(row.status,AvailabilityStatus::IdentityConflict);
                assert_eq!(row.reason,Some(AvailabilityReason::OriginMismatch));
            }
            assert!(matches!(all_first.probe_target(&f.config,&b),Err(InspectionError::ObservedIdentityConflict)));
            assert!(matches!(target_first.probe_target(&f.config,&b),Err(InspectionError::ObservedIdentityConflict)));
            assert!(target_first.observe_all(&f.config).unwrap().observed_conflict);
        });
        // Explicit fixture restoration is a harness fault reversal, not product
        // rebind, autoplay or a policy for resuming a session after a conflict.
        fs::remove_file(&roots).unwrap();fs::rename(&moved,&roots).unwrap();
        f.readonly(||{
            assert!(matches!(all_first.probe_target(&f.config,&b),Err(InspectionError::ObservedIdentityConflict)));
            let fresh=f.inspect().unwrap();
            assert!(fresh.observe_all(&f.config).unwrap().sources.iter().all(|r|r.status==AvailabilityStatus::Available));
            assert_eq!(fresh.probe_target(&f.config,&format!("{b}/b.flac")).unwrap().source_id(),b);
        });
    }

    #[test]
    fn source_cover_terminal_separator_keeps_single_and_batch_reads_for_all_sources() {
        use std::io::Read;
        let f=Fixture::new(false);let a=f.id(&f.a);let b=f.id(&f.b);
        fs::write(f.a.join("cover.jpg"),b"A").unwrap();
        fs::write(f.b.join("cover.jpg"),b"B").unwrap();
        f.readonly(||{
            let coordinator=crate::config_save_operation::Coordinator::new();
            let transaction=coordinator.try_begin_background().unwrap();
            let read=|target:&crate::local_media::SourceCoverTarget<'_>|->Result<Vec<u8>,String>{
                let cover=target.open_cover("cover.jpg")?.ok_or("missing own cover")?;
                let mut bytes=vec![0;1];let mut reader=cover.reader();
                reader.read_exact(&mut bytes).map_err(|_|"own fixture read failed")?;
                cover.validate()?;Ok(bytes)
            };
            // A trailing separator on B must not disable A's cover either.
            for suffixes in [[false,false],[false,true],[true,false],[true,true]] {
                let mut config=f.config.clone();
                for (path,suffix) in config.iter_mut().zip(suffixes) {if suffix {path.push('/');}}
                for (id,expected) in [(&a,b"A".to_vec()),(&b,b"B".to_vec())] {
                    let bytes=crate::local_media::with_source_cover_target(
                        &f._lease,&transaction,&coordinator,&f.library,&config,id,|target|read(target)).unwrap();
                    assert_eq!(bytes,expected);
                }
                let inventory=PreliminaryInventory::inspect(&f.library,&config).unwrap();
                let covers=inventory.read_cover_pair(&f._lease,&transaction,&coordinator,
                    &f.library,&config,&[a.clone(),b.clone()],|_,target|read(target)).unwrap();
                assert_eq!(covers,vec![Ok(b"A".to_vec()),Ok(b"B".to_vec())]);
            }
        });
    }

    #[test]
    fn source_cover_terminal_separator_keeps_duplicate_and_alias_refusal_before_read() {
        let f=Fixture::new(false);let a=f.id(&f.a);
        let alias=f.root.join("alias-A");symlink(&f.a,&alias).unwrap();
        f.readonly(||{
            let coordinator=crate::config_save_operation::Coordinator::new();
            let transaction=coordinator.try_begin_background().unwrap();
            let calls=std::cell::Cell::new(0);
            let mut duplicate=f.config.clone();duplicate[1]=format!("{}/",duplicate[0]);
            let mut aliased=f.config.clone();aliased[0]=format!("{}/",alias.to_str().unwrap());
            let mut dotted=f.config.clone();dotted[0]=format!("{}/./",dotted[0]);
            for config in [duplicate,aliased,dotted] {
                let result=crate::local_media::with_source_cover_target(
                    &f._lease,&transaction,&coordinator,&f.library,&config,&a,
                    |_|{calls.set(calls.get()+1);Ok(())});
                assert_eq!(result.unwrap_err(),"LOCAL_COVER_CONTEXT_CHANGED");
            }
            assert_eq!(calls.get(),0);
        });
    }

    // Consumer component regression: actual Lease, Coordinator transaction,
    // D03 snapshots and physical reader. Not AppHandle/IPC/startup/MPD proof.
    #[test]
    fn source_cover_offline_a_b_read_alias_refusal_and_size_bound() {
        use std::os::unix::fs::symlink;
        for scene in 0..4 {
            let f = Fixture::new(false);
            let b = f.id(&f.b);
            fs::rename(&f.a, f.root.join("A-away")).unwrap();
            let cover = f.b.join("cover.jpg");
            match scene {
                0 => fs::write(&cover, b"B").unwrap(),
                1 => {
                    let outside = f.root.join("outside.jpg");
                    fs::write(&outside, b"outside source B").unwrap();
                    symlink(&outside, &cover).unwrap();
                }
                2 => symlink(f.root.join("absent-outside.jpg"), &cover).unwrap(),
                3 => {
                    let file = fs::File::create(&cover).unwrap();
                    file.set_len(8 * 1024 * 1024 + 1).unwrap();
                    fs::write(f.b.join("Folder.jpg"), b"later").unwrap();
                }
                _ => unreachable!(),
            }
            f.readonly(|| {
                let coordinator = crate::config_save_operation::Coordinator::new();
                let transaction = coordinator.try_begin_background().unwrap();
                let inventory = f.inspect().unwrap();
                let verify = || -> Result<(), String> {
                    f._lease.validate()?;
                    if !transaction.belongs_to(&coordinator) || coordinator.shutdown_requested() {
                        return Err("own consumer reservation unavailable".into());
                    }
                    inventory.revalidate(&f.config)
                        .map_err(|_| "own source snapshot changed".to_string())
                };
                let socket = f.root.join("never-started-mpd.socket");
                let result = crate::local_media::with_source_cover_target(
                    &f._lease, &transaction, &coordinator, &f.library, &f.config, &b,
                    |target| crate::audio::AudioEngine::get_local_cover_from_source(
                        socket.to_str().unwrap(), &b, target, &verify,
                    ),
                );
                match scene {
                    0 => assert_eq!(result.unwrap().as_deref(), Some("data:image/jpeg;base64,Qg==")),
                    1 => assert_eq!(result.unwrap_err(), "LOCAL_COVER_OUTSIDE_SOURCE"),
                    2 => assert_eq!(result.unwrap_err(), "LOCAL_COVER_SOURCE_UNAVAILABLE"),
                    3 => assert_eq!(result.unwrap().as_deref(), Some("data:image/jpeg;base64,bGF0ZXI=")),
                    _ => unreachable!(),
                }
            });
        }
    }


    #[test]
    fn repeated_cover_candidate_checks_keep_final_topology_refusal() {
        for changed in [false,true] {
            let f=Fixture::new_with_extra_b(false,1500);
            let b=f.id(&f.b);
            fs::write(f.b.join("cover.png"),b"own cover bytes").unwrap();
            let coordinator=crate::config_save_operation::Coordinator::new();
            let transaction=coordinator.try_begin_background().unwrap();
            let start=std::time::Instant::now();
            let result=crate::local_media::with_source_cover_target(
                &f._lease,&transaction,&coordinator,&f.library,&f.config,&b,
                |target| {
                    for _ in 0..16 {assert!(target.open_cover("absent.jpg")?.is_none());}
                    let file=target.open_cover("cover.png")?.unwrap();
                    let mut bytes=Vec::new();let mut reader=file.reader();
                    std::io::Read::read_to_end(&mut reader,&mut bytes).unwrap();
                    file.validate()?;
                    if changed {
                        // Change an unrelated published entry after reading the
                        // cover. The final full snapshot must still refuse it.
                        let entry=f.library.join(&b).join("b.flac");
                        fs::remove_file(&entry).unwrap();
                        symlink(f.a.join("a.flac"),&entry).unwrap();
                    }
                    Ok(bytes)
                });
            eprintln!("COVER_SCALE changed={changed} entries=1500 elapsed_ms={}",start.elapsed().as_millis());
            if changed {assert_eq!(result.unwrap_err(),"LOCAL_COVER_CONTEXT_CHANGED");}
            else {assert_eq!(result.unwrap(),b"own cover bytes");}
            assert_eq!(fs::read(f.b.join("cover.png")).unwrap(),b"own cover bytes");
            assert!(f._lease.validate().is_ok());
        }
    }

    #[test]
    fn private_config_read_preserves_existing_source_snapshot() {
        let f=Fixture::new(false);
        let mut cfg=crate::config::AppConfig::default();cfg.local_folders=f.config.clone();cfg.first_run=false;
        let file=f.profile.join("config.json");fs::write(&file,serde_json::to_vec(&cfg).unwrap()).unwrap();
        fs::set_permissions(&file,fs::Permissions::from_mode(0o600)).unwrap();
        let inventory=f.inspect().unwrap();
        let loaded=crate::config::AppConfig::load_from_path(&file).unwrap();
        assert_eq!(loaded,cfg);
        let after=inventory.observe_all(&f.config);
        eprintln!("PRIVATE_CONFIG_READ source_snapshot_error={:?}",after.as_ref().err());
        assert!(after.is_ok(),"reading an already-private config must not invalidate the source snapshot");
    }
    #[test]
    fn retained_mirror_revalidation_rejects_added_removed_and_retargeted_entries() {
        for scene in 0..3 {
            let f=Fixture::new_with_extra_b(false,1500);
            let b=f.id(&f.b);
            let snapshot=crate::local_media::mirror::ReadonlyMirrorSnapshot::open(&f.library).unwrap();
            let old_start=std::time::Instant::now();
            for _ in 0..8 {snapshot.revalidate_by_rebuilding_for_comparison().unwrap();}
            let old_ms=old_start.elapsed().as_millis();
            let start=std::time::Instant::now();
            for _ in 0..8 {snapshot.revalidate().unwrap();}
            eprintln!("MIRROR_REVALIDATION entries=1500 passes=8 old_ms={old_ms} new_ms={}",start.elapsed().as_millis());
            let entry=f.library.join(&b).join("b.flac");
            match scene {
                0=>symlink(f.b.join("b.flac"),f.library.join(&b).join("added.flac")).unwrap(),
                1=>fs::remove_file(&entry).unwrap(),
                _=>{fs::remove_file(&entry).unwrap();symlink(f.a.join("a.flac"),&entry).unwrap();}
            }
            assert!(snapshot.revalidate().is_err());
            assert!(snapshot.revalidate_by_rebuilding_for_comparison().is_err());
        }
    }

    #[test]
    fn paired_covers_share_inventory_and_refuse_the_entire_changed_snapshot() {
        for changed in [false,true] {
            let f=Fixture::new_with_extra_b(false,1500);let b=f.id(&f.b);
            fs::write(f.b.join("cover.png"),b"pair own bytes").unwrap();
            let coordinator=crate::config_save_operation::Coordinator::new();
            let transaction=coordinator.try_begin_background().unwrap();
            let paths=vec![b.clone(),format!("{b}/b.flac")];
            let inventory=f.inspect().unwrap();let before=inventory.observe_all(&f.config).unwrap();
            let verify=||->Result<(),String> {
                f._lease.validate()?;
                if !transaction.belongs_to(&coordinator) || coordinator.shutdown_requested() {return Err("own reservation changed".into());}
                inventory.identity.revalidate().map_err(|_|"own identity changed")?;
                inventory.mirror.revalidate_binding().map_err(|_|"own marker changed".to_string())
            };
            let mut reads=0;let start=std::time::Instant::now();
            let result=inventory.read_cover_pair(&f._lease,&transaction,&coordinator,&f.library,&f.config,&paths,|path,target| {
                let value=crate::audio::AudioEngine::get_local_cover_from_source(
                    f.root.join("never-started.socket").to_str().unwrap(),path,target,&verify)?;
                reads+=1;
                if changed && reads==1 {
                    let entry=f.library.join(&b).join("b.flac");fs::remove_file(&entry).unwrap();symlink(f.a.join("a.flac"),entry).unwrap();
                }
                Ok(value)
            });
            eprintln!("COVER_PAIR changed={changed} entries=1500 reads={reads} elapsed_ms={}",start.elapsed().as_millis());
            if changed {assert_eq!(result.unwrap_err(),"LOCAL_COVER_CONTEXT_CHANGED");}
            else {
                let results=result.unwrap();assert_eq!(results.len(),2);
                for item in results {assert_eq!(item.unwrap().as_deref(),Some("data:image/png;base64,cGFpciBvd24gYnl0ZXM="));}
                let after=inventory.observe_all(&f.config).unwrap();assert_eq!(before.sources,after.sources);
                assert!(inventory.read_cover_pair(&f._lease,&transaction,&coordinator,&f.library,&f.config,&[],|_,_|Ok(())).is_err());
                assert!(inventory.read_cover_pair(&f._lease,&transaction,&coordinator,&f.library,&f.config,&vec![b.clone();3],|_,_|Ok(())).is_err());
            }
        }
    }

    #[test]
    fn retained_read_scope_consolidates_scans_and_rejects_changes_after_callback_checks() {
        for entries in [1500,6000] {
            for changed in [false,true] {
                let f=Fixture::new_with_extra_b(false,entries);let b=f.id(&f.b);
                fs::write(f.b.join("cover.png"),b"scope").unwrap();
                fs::rename(&f.a,f.root.join("A-offline")).unwrap();
                let coordinator=crate::config_save_operation::Coordinator::new();
                let transaction=coordinator.try_begin_background().unwrap();
                let paths=vec![b.clone(),format!("{b}/b.flac")];
                let read_cover=|inventory:&PreliminaryInventory,path:&str,target:&local_media::SourceCoverTarget<'_>| {
                    let verify=||->Result<(),String> {
                        f._lease.validate()?;
                        if !transaction.belongs_to(&coordinator) || coordinator.shutdown_requested() {return Err("own reservation changed".into());}
                        inventory.identity.revalidate().map_err(|_|"own identity changed")?;
                        inventory.mirror.revalidate_binding().map_err(|_|"own marker changed".to_string())
                    };
                    crate::audio::AudioEngine::get_local_cover_from_source(
                        f.root.join("never-started.socket").to_str().unwrap(),path,target,&verify)
                };
                let old_start=std::time::Instant::now();
                let old=f.inspect().unwrap();let old_before=old.observe_all(&f.config).unwrap();
                let old_values=old.read_cover_pair(&f._lease,&transaction,&coordinator,&f.library,&f.config,&paths,
                    |path,target|read_cover(&old,path,target)).unwrap();
                let old_after=old.observe_all(&f.config).unwrap();assert_eq!(old_before.sources,old_after.sources);
                let old_ms=old_start.elapsed().as_millis();
                let start=std::time::Instant::now();
                let result=with_read_inventory(&f.library,&f.config,|before,recheck,inventory| {
                    assert!(before.sources.iter().any(|row|row.status==AvailabilityStatus::Unavailable));
                    assert!(!before.observed_conflict);
                    let values=inventory.read_cover_pair_in_context(&f._lease,&transaction,&coordinator,
                        &f.library,&f.config,&paths,|path,target|read_cover(inventory,path,target))?;
                    let after=recheck()?;assert_eq!(before.sources,after.sources);
                    if changed {
                        // Deliberately after both physical reads and their final
                        // origin observations: only the enclosing full inventory
                        // fence can still prevent these values from escaping.
                        let entry=f.library.join(&b).join("b.flac");fs::remove_file(&entry).unwrap();
                        symlink(f.a.join("a.flac"),entry).unwrap();
                    }
                    Ok(values)
                });
                eprintln!("READ_SCOPE entries={entries} changed={changed} previous_pair_ms={old_ms} consolidated_pair_ms={}",start.elapsed().as_millis());
                if changed {assert!(matches!(result,Err(ref error) if error=="SOURCE_AVAILABILITY_CHANGED"));}
                else {
                    let (values,observation)=result.unwrap();assert_eq!(observation.sources.len(),2);assert_eq!(values,old_values);
                    for value in values {assert_eq!(value.unwrap().as_deref(),Some("data:image/png;base64,c2NvcGU="));}
                }
                assert!(f._lease.validate().is_ok());
            }
        }
    }

    #[test]
    fn watched_mirror_reuses_proof_and_rejects_changes_before_publication() {
        for scene in 0..8 {
            let f=Fixture::new_with_extra_b(false,if scene==0 {6000}else{30});let b=f.id(&f.b);
            fs::write(f.b.join("cover.png"),b"watch own image bytes").unwrap();
            fs::rename(&f.a,f.root.join("A-offline")).unwrap();
            let cache=MirrorReadCache::default();
            let initial=cache.get(&f.library).unwrap();
            assert!(initial.has_change_watch(),"own local fixture must exercise actual kernel notifications");
            let coordinator=crate::config_save_operation::Coordinator::new();
            let transaction=coordinator.try_begin_background().unwrap();
            let paths=vec![b.clone(),format!("{b}/b.flac")];
            let start=std::time::Instant::now();
            for pass in 0..if scene==0 {16}else{1} {
                let result=with_cached_read_inventory(&cache,&f.library,&f.config,|before,recheck,inventory| {
                    assert!(Arc::ptr_eq(&initial,&inventory.mirror));
                    assert!(before.sources.iter().any(|r|r.status==AvailabilityStatus::Unavailable));
                    let verify=||->Result<(),String> {
                        f._lease.validate()?;
                        if !transaction.belongs_to(&coordinator) {return Err("transaction changed".into());}
                        inventory.identity.revalidate().map_err(|_|"identity changed")?;
                        inventory.mirror.revalidate_binding().map_err(|_|"marker changed".into())
                    };
                    let values=inventory.read_cover_pair_in_context(&f._lease,&transaction,&coordinator,
                        &f.library,&f.config,&paths,|path,target|crate::audio::AudioEngine::get_local_cover_from_source(
                            f.root.join("never-started.socket").to_str().unwrap(),path,target,&verify))?;
                    assert_eq!(before.sources,recheck()?.sources);
                    let link=f.library.join(&b).join("b.flac");
                    // Every mutation occurs AFTER actual physical reads and the
                    // callback's last origin observation, before final return.
                    match scene {
                        0=>{},
                        1=>symlink(f.b.join("b.flac"),f.library.join(&b).join("new.flac")).unwrap(),
                        2=>fs::remove_file(&link).unwrap(),
                        3=>{fs::remove_file(&link).unwrap();symlink(f.a.join("a.flac"),&link).unwrap();},
                        4=>{fs::rename(f.library.join(&b),f.library.join(format!("{b}-away"))).unwrap();},
                        5=>{fs::hard_link(&link,f.root.join("external-hardlink")).unwrap();},
                        6=>inventory.mirror.remove_change_watch(),
                        7=>{let marker=f.library.join(local_media::mirror::MANIFEST_NAME);let bytes=fs::read(&marker).unwrap();fs::write(marker,bytes).unwrap();},
                        _=>unreachable!(),
                    }
                    Ok(values)
                });
                if scene==0 {
                    let (values,_)=result.unwrap();assert_eq!(values.len(),2);
                    for value in values {assert!(value.unwrap().unwrap().starts_with("data:image/png;base64,"));}
                } else {
                    assert!(matches!(result,Err(ref e) if e=="SOURCE_AVAILABILITY_CHANGED"),"scene={scene}");
                    assert!(initial.revalidate().is_err());
                    assert!(initial.revalidate().is_err(),"dirty proof stays invalid after its events were drained");
                }
                if pass==15 {eprintln!("WATCHED_COVER_SCOPE entries=6000 requests=16 physical_reads=32 warm_total_us={}",start.elapsed().as_micros());}
            }
            f._lease.validate().unwrap();
        }
        // A different published profile never reuses the previous proof.
        let cache=MirrorReadCache::default();let a=Fixture::new(false);let b=Fixture::new(false);
        let first=cache.get(&a.library).unwrap();let second=cache.get(&b.library).unwrap();
        assert!(!Arc::ptr_eq(&first,&second));
    }

    #[test]
    fn unknown_mirror_schema_is_distinct_without_reading_or_changing_originals() {
        for scene in 0..6 {
            let f=Fixture::new(false);
            let marker=f.library.join(local_media::mirror::MANIFEST_NAME);
            let mut manifest:serde_json::Value=serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
            match scene {
                0=>manifest["schema_version"]=99.into(),
                3=>manifest["generation_id"]="".into(),
                4=>{manifest["schema_version"]=1.into();manifest["registry_id"]=serde_json::Value::Null;},
                _=>{},
            }
            let bytes=match scene {
                1=>b"{".to_vec(),
                2=>{let mut bytes=b"{\"schema_version\":99,".to_vec();
                    bytes.extend_from_slice(&serde_json::to_vec(&manifest).unwrap()[1..]);bytes},
                _=>serde_json::to_vec(&manifest).unwrap(),
            };
            fs::write(&marker,&bytes).unwrap();
            let before=f.inventory();let calls=Cell::new(0);
            let result=with_cached_read_inventory(&MirrorReadCache::default(),&f.library,&f.config,|_,_,_| {
                calls.set(calls.get()+1);Ok(())
            });
            match scene {
                0=>assert!(matches!(result,Err(ref e) if e=="SOURCE_AVAILABILITY_UNSUPPORTED_FORMAT")),
                5=>assert!(result.is_ok()),
                _=>assert!(matches!(result,Err(ref e) if e=="SOURCE_AVAILABILITY_CHANGED")),
            }
            assert_eq!(calls.get(),if scene==5 {1}else{0});
            assert_eq!(f.inventory(),before);f._lease.validate().unwrap();
        }
        // Dirty topology with the original marker binding retained must fail
        // CHANGED before the read callback. Changing marker bytes between
        // requests instead starts a fresh capture and classification.
        let f=Fixture::new(false);let cache=MirrorReadCache::default();
        let proof=cache.get(&f.library).unwrap();
        let id=f.id(&f.b);
        fs::write(f.library.join(id).join("unlisted.flac"),b"own topology mutation").unwrap();
        let before=f.inventory();let calls=Cell::new(0);
        let result=with_cached_read_inventory(&cache,&f.library,&f.config,|_,_,_| {calls.set(1);Ok(())});
        assert!(matches!(result,Err(ref e) if e=="SOURCE_AVAILABILITY_CHANGED"));
        assert_eq!(calls.get(),0);assert!(proof.revalidate().is_err());assert_eq!(f.inventory(),before);
    }

}
