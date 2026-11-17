//! Bounded callers around potentially non-cancellable filesystem calls.
//! A timed-out worker retains its physical permit and owned handles until it
//! actually returns. Only the caller may publish an accepted result.
use std::sync::{atomic::{AtomicBool,AtomicUsize,Ordering},mpsc,Arc,Mutex,OnceLock};
use std::time::Duration;
#[path="windows_io_watch.rs"]
pub(crate) mod watch;
#[path="windows_io_retire.rs"]
pub(crate) mod retire;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Error { Busy, Timeout, WorkerFailed }
#[derive(Clone)]
pub(crate) struct Job(Arc<AtomicBool>);
impl Job {
    pub(crate) fn current(&self)->bool { !self.0.load(Ordering::Acquire) }
}
pub(crate) struct Pool { active:AtomicUsize,limit:usize }
struct Permit(Arc<Pool>);
impl Drop for Permit { fn drop(&mut self){self.0.active.fetch_sub(1,Ordering::AcqRel);} }
type Slot<T>=Arc<Mutex<Option<T>>>;
struct Reply<T>{receiver:Option<mpsc::Receiver<Slot<T>>>,released:Option<mpsc::Sender<()>>}
impl<T> Drop for Reply<T>{fn drop(&mut self){
    // First discard any late queued Arc, while the worker still retains its
    // own Arc. Only then release the worker to drop a rejected payload there.
    drop(self.receiver.take());drop(self.released.take());
}}
fn deliver<T>(send:mpsc::SyncSender<Slot<T>>,released:mpsc::Receiver<()>,value:T){
    let slot=Arc::new(Mutex::new(Some(value)));
    let sent=send.send(slot.clone());
    if sent.is_ok(){let _=released.recv();}
    drop(sent);drop(slot);
}
impl Pool {
    fn new(limit:usize)->Arc<Self>{Arc::new(Self{active:AtomicUsize::new(0),limit})}
    fn run<T:Send+'static>(self:&Arc<Self>,timeout:Duration,work:impl FnOnce(Job)->T+Send+'static)->Result<T,Error>{
        if timeout.is_zero(){return Err(Error::Timeout);}
        self.active.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<self.limit).then_some(n+1)).map_err(|_|Error::Busy)?;
        let permit=Permit(self.clone());
        let job=Job(Arc::new(AtomicBool::new(false)));let cancel=job.clone();
        let (send,receive)=mpsc::sync_channel(1);let (finished,released)=mpsc::channel();
        let reply=Reply{receiver:Some(receive),released:Some(finished)};
        std::thread::Builder::new().name("sonante-file-read".into()).spawn(move||{
            let _permit=permit;
            let result=work(job);
            // Keep ownership through the caller's receive/drop decision, even
            // if send wins the timeout race and puts a File in the mailbox.
            deliver(send,released,result);
        }).map_err(|_|Error::WorkerFailed)?;
        let result=match reply.receiver.as_ref().unwrap().recv_timeout(timeout){
            Ok(slot)=>{let value=slot.lock().map_err(|_|Error::WorkerFailed).and_then(|mut value|value.take().ok_or(Error::WorkerFailed));value},
            Err(mpsc::RecvTimeoutError::Timeout)=>Err(Error::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected)=>Err(Error::WorkerFailed),
        };
        cancel.0.store(true,Ordering::Release);
        drop(reply);
        result
    }
}
pub(crate) const READ_TIMEOUT:Duration=Duration::from_secs(3);
pub(crate) fn read<T:Send+'static>(timeout:Duration,work:impl FnOnce(Job)->T+Send+'static)->Result<T,Error>{
    static POOL:OnceLock<Arc<Pool>>=OnceLock::new();
    POOL.get_or_init(||Pool::new(8)).run(timeout,work)
}

#[cfg(test)]mod tests{
    use super::*;
    fn empty(pool:&Pool){let end=std::time::Instant::now()+Duration::from_secs(2);while pool.active.load(Ordering::Acquire)!=0{assert!(std::time::Instant::now()<end);std::thread::yield_now();}}
    #[test]fn accepted_result_and_task_error_are_not_rewritten(){let p=Pool::new(1);assert_eq!(p.run(Duration::from_secs(1),|j|{assert!(j.current());Err::<(),_>("own-error")}),Ok(Err("own-error")));empty(&p);}
    #[test]fn timed_out_call_retains_physical_capacity_and_late_result_is_dropped(){
        let p=Pool::new(1);let (release,wait)=mpsc::channel();let (started,ready)=mpsc::channel();let dropped=Arc::new(AtomicBool::new(false));
        struct Owned(Arc<AtomicBool>);impl Drop for Owned{fn drop(&mut self){self.0.store(true,Ordering::Release);}}
        let d=dropped.clone();let q=p.clone();
        let caller=std::thread::spawn(move||q.run(Duration::from_millis(50),move|j|{started.send(()).unwrap();wait.recv().unwrap();assert!(!j.current());Owned(d)}).err());
        ready.recv_timeout(Duration::from_secs(1)).unwrap();assert_eq!(caller.join().unwrap(),Some(Error::Timeout));
        assert_eq!(p.active.load(Ordering::Acquire),1);assert!(matches!(p.run(Duration::from_secs(1),|_|()),Err(Error::Busy)));assert!(!dropped.load(Ordering::Acquire));
        release.send(()).unwrap();empty(&p);assert!(dropped.load(Ordering::Acquire));assert_eq!(p.run(Duration::from_secs(1),|_|7),Ok(7));empty(&p);
    }
    #[test]fn panicked_worker_fails_without_leaking_capacity(){let p=Pool::new(1);assert_eq!(p.run(Duration::from_secs(1),|_|panic!("own synthetic worker")),Err(Error::WorkerFailed));empty(&p);}
    #[test]fn zero_deadline_never_starts_a_worker(){let p=Pool::new(1);assert_eq!(p.run(Duration::ZERO,|_|panic!("must not run")),Err(Error::Timeout));assert_eq!(p.active.load(Ordering::Acquire),0);}
    #[test]fn a_late_mailbox_payload_is_destroyed_by_its_worker_not_the_caller(){
        let dropped=Arc::new(Mutex::new(None));struct Owned(Arc<Mutex<Option<std::thread::ThreadId>>>);
        impl Drop for Owned{fn drop(&mut self){*self.0.lock().unwrap()=Some(std::thread::current().id());}}
        let (send,receive)=mpsc::sync_channel(1);let(finished,released)=mpsc::channel();
        let reply=Reply{receiver:Some(receive),released:Some(finished)};let d=dropped.clone();
        let worker=std::thread::spawn(move||deliver(send,released,Owned(d)));let worker_id=worker.thread().id();
        // Simulate a timeout that has rejected this result although delivery
        // has already won its race with closing the response mailbox.
        let late=reply.receiver.as_ref().unwrap().recv_timeout(Duration::from_secs(1)).unwrap();
        drop(late);assert!(dropped.lock().unwrap().is_none());drop(reply);worker.join().unwrap();
        assert_eq!(*dropped.lock().unwrap(),Some(worker_id));assert_ne!(worker_id,std::thread::current().id());
    }
}
