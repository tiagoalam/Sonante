//! The previous source is retained until candidate playback is confirmed.
use super::protocol::{Playback, Result};

pub trait Route {
    fn observe(&mut self) -> Result<Playback>;
    fn stop(&mut self) -> Result<()>;
    fn start(&mut self) -> Result<()>;
    fn seek(&mut self, seconds: f64) -> Result<()>;
    fn resume_at(&mut self, seconds: f64) -> Result<()>;
    fn pause(&mut self) -> Result<()>;
    fn abort(&mut self) -> Result<()>;
    fn suspend_output(&mut self) -> Result<()> { Ok(()) }
    fn restore_output(&mut self) -> Result<()> { Ok(()) }
    fn hold_for_replacement(&mut self)->Result<()> {self.stop()}
    fn restore_held(&mut self,saved:&Playback)->Result<()> where Self:Sized {restore(self,saved)}
}

fn restore(route: &mut impl Route, saved: &Playback) -> Result<()> {
    if saved.state == "stop" {
        return route.stop();
    }
    route.resume_at(saved.elapsed)?;
    if saved.state == "pause" {
        route.pause()?;
    }
    let observed = route.observe()?;
    if observed.state != saved.state {
        return Err("rollback_failed");
    }
    Ok(())
}

pub fn activate<T: Route>(candidate: &mut T, mut previous: Option<&mut T>) -> Result<Playback> {
    activate_guarded(candidate,previous.take(),||true)
}
pub fn activate_guarded<T:Route>(candidate:&mut T,mut previous:Option<&mut T>,current:impl Fn()->bool)->Result<Playback>{
    activate_committed(candidate,previous.take(),current,|_,_|Ok(()))
}
/// Keep the old generation recoverable until the caller confirms its durable
/// publication too. Failure terminates the candidate before restoring the old
/// source; a superseding intent never resumes it.
pub fn activate_committed<T:Route>(candidate:&mut T,mut previous:Option<&mut T>,current:impl Fn()->bool,commit:impl FnOnce(&mut T,&Playback)->Result<()>)->Result<Playback>{
    if !current(){return Err("PLAYBACK_SUPERSEDED");}
    let saved = previous.as_deref_mut().map(Route::observe).transpose()?;
    if let Some(old) = previous
        .as_deref_mut()
        .filter(|_| saved.as_ref().is_some_and(|s| s.state == "play"))
    {
        if let Err(cause) = old.hold_for_replacement() {
            if !current(){old.pause().map_err(|_|"rollback_failed")?;return Err("PLAYBACK_SUPERSEDED");}
            if old.restore_held(saved.as_ref().unwrap()).is_err() {
                return Err("rollback_failed");
            }
            if !current(){old.pause().map_err(|_|"rollback_failed")?;return Err("PLAYBACK_SUPERSEDED");}
            return Err(cause);
        }
    }
    // An exclusive paused route can still retain its audio client. Release only
    // its owned output without Play/Seek or changing its paused queue cursor.
    if let Some(old)=previous.as_deref_mut(){
        if let Err(cause)=old.suspend_output(){
            candidate.abort().map_err(|_|"rollback_failed")?;
            old.restore_output().map_err(|_|"rollback_failed")?;
            if let Some(saved)=saved.as_ref().filter(|s|s.state=="play"){
                if current(){old.restore_held(saved).map_err(|_|"rollback_failed")?;}
                if !current(){old.pause().map_err(|_|"rollback_failed")?;return Err("PLAYBACK_SUPERSEDED");}
            }
            return Err(cause);
        }
    }
    if !current(){
        candidate.abort().map_err(|_|"rollback_failed")?;
        if let Some(old)=previous{old.restore_output().map_err(|_|"rollback_failed")?;}
        return Err("PLAYBACK_SUPERSEDED");
    }
    let activated = candidate
        .start()
        .and_then(|_| candidate.observe())
        .and_then(|state| {
            if state.state == "play" {
                Ok(state)
            } else {
                Err("playback_unconfirmed")
            }
        });
    // A newer intent may arrive while the old source is held or the play ACK
    // is in flight. Kill the candidate before publishing/resuming anything.
    if !current(){
        candidate.abort().map_err(|_|"rollback_failed")?;
        if let Some(old)=previous{old.restore_output().map_err(|_|"rollback_failed")?;}
        return Err("PLAYBACK_SUPERSEDED");
    }
    let activated=activated.and_then(|state|{commit(candidate,&state)?;Ok(state)});
    match activated {
        Ok(state) => Ok(state),
        Err(cause) => {
            // A failed response can follow an accepted play command. Terminate
            // the owned candidate before any old source is resumed.
            candidate.abort().map_err(|_| "rollback_failed")?;
            if let Some(old)=previous {
                old.restore_output().map_err(|_|"rollback_failed")?;
                if let Some(saved)=saved.as_ref().filter(|s|s.state=="play"){
                    if current(){old.restore_held(saved).map_err(|_| "rollback_failed")?;}
                    if !current(){old.pause().map_err(|_|"rollback_failed")?;return Err("PLAYBACK_SUPERSEDED");}
                }
            }
            Err(cause)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        value: Playback,
        fail_start: bool,
        aborted: bool,
        seeked: f64,
        fail_abort: bool,
    }
    impl Fake {
        fn new(state: &str, fail_start: bool) -> Self {
            Self {
                value: Playback {
                    state: state.into(),
                    elapsed: 12.5,
                    duration: 90.0,
                    volume: 40,
                    audio: "44100:16:2".into(),
                },
                fail_start,
                aborted: false,
                seeked: 0.0,
                fail_abort: false,
            }
        }
    }
    impl Route for Fake {
        fn observe(&mut self) -> Result<Playback> {
            Ok(self.value.clone())
        }
        fn stop(&mut self) -> Result<()> {
            self.value.state = "stop".into();
            Ok(())
        }
        fn start(&mut self) -> Result<()> {
            if self.fail_start {
                Err("mpd_ack")
            } else {
                self.value.state = "play".into();
                Ok(())
            }
        }
        fn seek(&mut self, value: f64) -> Result<()> {
            self.seeked = value;
            Ok(())
        }
        fn resume_at(&mut self, value: f64) -> Result<()> {
            self.start()?;
            self.seek(value)
        }
        fn pause(&mut self) -> Result<()> {
            self.value.state = "pause".into();
            Ok(())
        }
        fn abort(&mut self) -> Result<()> {
            self.aborted = true;
            if self.fail_abort{return Err("fixture_abort_failed");}
            self.stop()
        }
    }
    #[test]
    fn failed_replacement_preserves_playing_paused_and_stopped() {
        for state in ["play", "pause", "stop"] {
            let mut old = Fake::new(state, false);
            let mut new = Fake::new("stop", true);
            assert_eq!(activate(&mut new, Some(&mut old)), Err("mpd_ack"));
            assert!(new.aborted);
            assert_eq!(old.value.state, state);
            if state == "play" {
                assert_eq!(old.seeked, 12.5);
            } else {
                assert_eq!(old.seeked, 0.0);
            }
        }
    }
    #[test]
    fn failed_rollback_is_explicit() {
        let mut old = Fake::new("play", true);
        let mut new = Fake::new("stop", true);
        assert_eq!(activate(&mut new, Some(&mut old)), Err("rollback_failed"));
        assert!(new.aborted);
    }
    #[test]
    fn successful_replacement_keeps_paused_previous_silent() {
        let mut old = Fake::new("pause", false);
        let mut new = Fake::new("stop", false);
        assert_eq!(activate(&mut new, Some(&mut old)).unwrap().state, "play");
        assert_eq!(old.value.state, "pause");
        assert_eq!(old.seeked, 0.0);
    }
    #[test]
    fn durable_failure_is_inside_the_cut_and_restores_only_a_previously_playing_route() {
        for state in ["play", "pause", "stop"] {
            let mut old=Fake::new(state,false);let mut next=Fake::new("stop",false);
            let mut writes=0;
            assert_eq!(activate_committed(&mut next,Some(&mut old),||true,|candidate,observed|{
                assert_eq!(observed.state,"play");assert!(!candidate.aborted);writes+=1;Err("queue_save_failed")
            }),Err("queue_save_failed"));
            assert_eq!(writes,1);assert!(next.aborted);assert_eq!(old.value.state,state);
            assert_eq!(old.seeked,if state=="play"{12.5}else{0.});
        }
    }
    #[test]
    fn durable_failure_after_a_newer_intent_never_autoplays_the_held_route() {
        use std::cell::Cell;
        let current=Cell::new(true);let mut old=Fake::new("play",false);let mut next=Fake::new("stop",false);
        assert_eq!(activate_committed(&mut next,Some(&mut old),||current.get(),|_,_|{
            current.set(false);Err("queue_save_failed")
        }),Err("PLAYBACK_SUPERSEDED"));
        assert!(next.aborted);assert_eq!(old.value.state,"pause");assert_eq!(old.seeked,0.);
    }
    #[test]
    fn uncertain_candidate_cleanup_never_resumes_the_old_route_after_a_write_failure() {
        let mut old=Fake::new("play",false);let mut next=Fake::new("stop",false);next.fail_abort=true;
        assert_eq!(activate_committed(&mut next,Some(&mut old),||true,|_,_|Err("queue_save_failed")),Err("rollback_failed"));
        assert!(next.aborted);assert_eq!(old.value.state,"stop");assert_eq!(old.seeked,0.);
    }
    #[test]
    fn publication_is_not_attempted_before_activation_or_after_supersession() {
        for admitted in [false,true]{
            let mut old=Fake::new("play",false);let mut next=Fake::new("stop",true);
            let mut writes=0;let result=activate_committed(&mut next,Some(&mut old),||admitted,|_,_|{writes+=1;Ok(())});
            assert_eq!(result,Err(if admitted{"mpd_ack"}else{"PLAYBACK_SUPERSEDED"}));assert_eq!(writes,0);
            assert_eq!(old.value.state,"play");
        }
    }
    #[test]
    fn paused_exclusive_route_releases_only_owned_output_and_rollback_reenables_without_play_or_seek(){
        use std::{cell::RefCell,rc::Rc};
        struct Exclusive{inner:Fake,old:bool,events:Rc<RefCell<Vec<&'static str>>>}
        impl Route for Exclusive{
            fn observe(&mut self)->Result<Playback>{self.inner.observe()}
            fn stop(&mut self)->Result<()>{self.events.borrow_mut().push("stop");self.inner.stop()}
            fn start(&mut self)->Result<()>{self.events.borrow_mut().push(if self.old{"old-play"}else{"candidate-play"});self.inner.start()}
            fn seek(&mut self,n:f64)->Result<()>{self.events.borrow_mut().push("seek");self.inner.seek(n)}
            fn resume_at(&mut self,n:f64)->Result<()>{self.events.borrow_mut().push("resume");self.inner.resume_at(n)}
            fn pause(&mut self)->Result<()>{self.inner.pause()}
            fn abort(&mut self)->Result<()>{self.events.borrow_mut().push("abort");self.inner.abort()}
            fn suspend_output(&mut self)->Result<()>{if self.old{self.events.borrow_mut().push("disable-owned-output");}Ok(())}
            fn restore_output(&mut self)->Result<()>{if self.old{self.events.borrow_mut().push("enable-owned-output");}Ok(())}
        }
        let events=Rc::new(RefCell::new(Vec::new()));let mut old=Exclusive{inner:Fake::new("pause",false),old:true,events:events.clone()};let mut candidate=Exclusive{inner:Fake::new("stop",true),old:false,events:events.clone()};
        assert_eq!(activate(&mut candidate,Some(&mut old)),Err("mpd_ack"));
        assert_eq!(*events.borrow(),["disable-owned-output","candidate-play","abort","enable-owned-output"]);
        assert_eq!(old.inner.value.state,"pause");assert_eq!(old.inner.value.elapsed,12.5);assert_eq!(old.inner.seeked,0.0);
    }
    #[test]
    fn pcm_replacement_rolls_back_retained_generation_without_stop_play_seek(){
        use std::{cell::RefCell,rc::Rc};
        struct Pcm{inner:Fake,old:bool,events:Rc<RefCell<Vec<&'static str>>>}
        impl Route for Pcm {
            fn observe(&mut self)->Result<Playback>{self.inner.observe()}
            fn stop(&mut self)->Result<()>{self.events.borrow_mut().push("terminal-stop");Err("pcm_terminal")}
            fn start(&mut self)->Result<()>{self.events.borrow_mut().push("candidate-play");self.inner.start()}
            fn seek(&mut self,_:f64)->Result<()>{self.events.borrow_mut().push("terminal-seek");Err("pcm_terminal")}
            fn resume_at(&mut self,_:f64)->Result<()>{self.events.borrow_mut().push("terminal-resume-seek");Err("pcm_terminal")}
            fn pause(&mut self)->Result<()>{self.inner.pause()}
            fn abort(&mut self)->Result<()>{self.events.borrow_mut().push("candidate-abort");self.inner.aborted=true;Ok(())}
            fn hold_for_replacement(&mut self)->Result<()>{assert!(self.old);self.events.borrow_mut().push("old-pause1");self.inner.pause()}
            fn restore_held(&mut self,saved:&Playback)->Result<()>{assert!(self.old);assert_eq!(self.inner.value.state,"pause");assert_eq!(saved.state,"play");self.events.borrow_mut().push("old-pause0");self.inner.value.state="play".into();Ok(())}
        }
        for fail_start in [false,true]{
            let events=Rc::new(RefCell::new(Vec::new()));
            let mut old=Pcm{inner:Fake::new("play",false),old:true,events:events.clone()};
            let mut candidate=Pcm{inner:Fake::new("stop",fail_start),old:false,events:events.clone()};
            let result=activate_committed(&mut candidate,Some(&mut old),||true,|_,_|Err("queue_save_failed"));
            assert_eq!(result,Err(if fail_start{"mpd_ack"}else{"queue_save_failed"}));
            assert_eq!(*events.borrow(),["old-pause1","candidate-play","candidate-abort","old-pause0"]);
            assert!(candidate.inner.aborted);assert_eq!(old.inner.value.state,"play");
            assert_eq!(old.inner.value.elapsed,12.5);assert_eq!(old.inner.seeked,0.);
        }
    }
    #[test]
    fn newer_intent_during_play_ack_aborts_candidate_and_never_resumes_the_previous_source(){
        use std::{cell::Cell,rc::Rc};
        struct Cancel{inner:Fake,current:Rc<Cell<bool>>,cancel_on_start:bool}
        impl Route for Cancel {
            fn observe(&mut self)->Result<Playback>{self.inner.observe()}
            fn stop(&mut self)->Result<()>{self.inner.stop()}
            fn start(&mut self)->Result<()>{let r=self.inner.start();if self.cancel_on_start{self.current.set(false);}r}
            fn seek(&mut self,n:f64)->Result<()>{self.inner.seek(n)}
            fn resume_at(&mut self,n:f64)->Result<()>{self.inner.resume_at(n)}
            fn pause(&mut self)->Result<()>{self.inner.pause()}
            fn abort(&mut self)->Result<()>{self.inner.abort()}
            fn hold_for_replacement(&mut self)->Result<()>{self.inner.pause()}
        }
        for fail in [false,true]{
            let current=Rc::new(Cell::new(true));
            let mut old=Cancel{inner:Fake::new("play",false),current:current.clone(),cancel_on_start:false};
            let mut next=Cancel{inner:Fake::new("stop",fail),current:current.clone(),cancel_on_start:true};
            assert_eq!(activate_guarded(&mut next,Some(&mut old),||current.get()),Err("PLAYBACK_SUPERSEDED"));
            assert!(next.inner.aborted);assert_eq!(old.inner.value.state,"pause");assert_eq!(old.inner.seeked,0.);
        }
    }
}
