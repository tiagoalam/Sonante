//! Physical desktop work admission, independent from network playback workers.
//! The permit moves into the real blocking closure, never the awaiting caller.
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
pub(super) struct Lane{active:Arc<AtomicUsize>,limit:usize}
pub(super) struct Permit{active:Arc<AtomicUsize>}
impl Lane{
    pub(super) fn new(limit:usize)->Self{Self{active:Arc::new(AtomicUsize::new(0)),limit}}
    pub(super) fn reserve(&self)->Option<Permit>{
        self.active.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<self.limit).then_some(n+1)).ok()?;
        Some(Permit{active:self.active.clone()})
    }
}
impl Drop for Permit{fn drop(&mut self){self.active.fetch_sub(1,Ordering::AcqRel);}}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn a_live_worker_retains_its_slot_when_its_caller_disappears(){
        let lane=Lane::new(1);let permit=lane.reserve().unwrap();let(release,wait)=std::sync::mpsc::channel();
        let worker=std::thread::spawn(move||{let _physical=permit;wait.recv().unwrap();});
        assert!(lane.reserve().is_none());release.send(()).unwrap();worker.join().unwrap();assert!(lane.reserve().is_some());
    }
    #[test]fn read_capacity_cannot_consume_write_admission_and_panic_releases_actual_ownership(){
        let read=Lane::new(4);let write=Lane::new(1);let held=(0..4).map(|_|read.reserve().unwrap()).collect::<Vec<_>>();assert!(read.reserve().is_none());
        let permit=write.reserve().unwrap();assert!(write.reserve().is_none());assert!(std::thread::spawn(move||{let _physical=permit;panic!("own worker failure");}).join().is_err());
        assert!(write.reserve().is_some());drop(held);assert!(read.reserve().is_some());
    }
}
