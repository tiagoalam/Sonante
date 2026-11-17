//! Cleanup acknowledgement is separate from the continuity fault already
//! recorded in the ring. A peer/probe failure is not a failure to terminate it.
pub(crate) fn stop_owned(
    signal:impl FnOnce()->Result<(),&'static str>,
    producer:impl FnOnce()->bool,
    consumer:impl FnOnce()->bool,
)->Result<(),&'static str>{
    let signalled=signal();
    // Never short-circuit before attempting both exact owned Jobs, including
    // a failed event signal or failure to terminate the first peer.
    let producer=producer();let consumer=consumer();
    if !producer||!consumer{Err("pcm_shutdown_failed")}else{signalled}
}
#[cfg(test)]mod tests{
    use super::*;
    use std::cell::{Cell,RefCell};
    #[test]
    fn expected_continuity_fault_remains_recorded_but_confirmed_owned_cleanup_succeeds(){
        let lost=Cell::new(false);let calls=RefCell::new(Vec::new());
        let result=stop_owned(||{lost.set(true);calls.borrow_mut().push("record-lost");Ok(())},||{calls.borrow_mut().push("close-producer");true},||{calls.borrow_mut().push("close-consumer");true});
        assert_eq!(result,Ok(()));assert!(lost.get());assert_eq!(*calls.borrow(),["record-lost","close-producer","close-consumer"]);
    }
    #[test]
    fn event_failure_still_attempts_both_jobs_and_is_not_hidden(){
        let calls=RefCell::new(Vec::new());
        assert_eq!(stop_owned(||Err("pcm_signal_failed"),||{calls.borrow_mut().push("producer");true},||{calls.borrow_mut().push("consumer");true}),Err("pcm_signal_failed"));
        assert_eq!(*calls.borrow(),["producer","consumer"]);
    }
    #[test]
    fn failed_peer_cleanup_never_acknowledges_silence_or_skips_the_other_peer(){
        for (producer,consumer)in[(false,true),(true,false),(false,false)]{
            let calls=RefCell::new(Vec::new());
            assert_eq!(stop_owned(||Ok(()),||{calls.borrow_mut().push("producer");producer},||{calls.borrow_mut().push("consumer");consumer}),Err("pcm_shutdown_failed"));
            assert_eq!(*calls.borrow(),["producer","consumer"]);
        }
    }
}
