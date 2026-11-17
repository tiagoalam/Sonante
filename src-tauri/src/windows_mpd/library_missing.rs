//! Ephemeral negative-catalog proof shared by favorites and playlists. Never
//! serialized, cached across requests or accepted as authority from a renderer.
use super::{Library,State,Path,PathBuf,Result};
pub(crate) struct Proof{epoch:u64,revision:u64,roots:Vec<PathBuf>,signature:Option<[u8;32]>,warnings:usize}
impl Proof{
    pub(super) fn capture(library:&Library)->Result<Self>{
        let proof=Self::capture_roots(library)?;
        if !proof.clean_catalog(){return Err("library_unavailable");}Ok(proof)
    }
    pub(super) fn capture_roots(library:&Library)->Result<Self>{
        let state=library.data.lock().map_err(|_|"library_state")?;
        let signature=state.signature;
        let proof=Self{epoch:state.status.epoch,revision:state.status.revision,roots:state.roots.clone(),signature,warnings:state.status.warnings};
        if !proof.matches(library,&state){return Err("library_unavailable");}Ok(proof)
    }
    pub(crate) fn clean_catalog(&self)->bool{self.warnings==0&&self.signature.is_some()}
    pub(super) fn version(&self,epoch:u64,revision:u64)->bool{self.epoch==epoch&&self.revision==revision}
    pub(super) fn matches(&self,library:&Library,state:&State)->bool{
        library.current(self.epoch)&&self.version(state.status.epoch,state.status.revision)
            &&state.sources.local&&state.status.phase=="ready"&&state.status.error.is_none()
            &&state.status.warnings==self.warnings&&!self.roots.is_empty()&&state.roots==self.roots&&state.signature==self.signature
    }
    pub(super) fn admit_with(&self,library:&Library,current:&dyn Fn()->bool,canonical:&impl Fn(&Path,&dyn Fn()->bool)->Result<PathBuf>)->Result<()>{
        let current=||current()&&library.current(self.epoch);
        for root in &self.roots{
            if !current(){return Err("library_changed");}
            if canonical(root,&current).as_ref()!=Ok(root){return Err("library_unavailable");}
            if !current(){return Err("library_changed");}
        }Ok(())
    }
    pub(crate) fn validate(&self,library:&Library)->Result<()>{
        let state=library.data.lock().map_err(|_|"library_state")?;
        if self.matches(library,&state){Ok(())}else{Err("library_changed")}
    }
}
