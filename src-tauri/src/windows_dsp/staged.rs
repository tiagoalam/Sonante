//! A queue/cursor prepared on a fresh, stopped engine. A paused route switch
//! must not play then pause merely to materialize a native seek cursor. The
//! saved position is admitted only on the next explicit start/resume.
use crate::windows_mpd::protocol::{Playback,Result};
#[derive(Clone)]
pub(crate) struct Staged {state:Playback}
impl Staged {
    /// Construct the stopped cursor before effects, but do not publish it until
    /// both command acknowledgements arrive. No Play/Seek/retry on failure.
    pub(crate) fn stop_after(before:Option<&Playback>,mut send:impl FnMut(&str)->Result<()>)->Result<Option<Self>>{
        let stopped=before.map(|state|{let mut next=Self::new(state)?;next.state.state="stop".into();next.state.elapsed=0.;Ok::<_,&'static str>(next)}).transpose()?;
        send("stop")?;send("clearerror")?;Ok(stopped)
    }
    pub fn new(before:&Playback)->Result<Self>{
        if !before.elapsed.is_finite()||before.elapsed<0.||!before.duration.is_finite()||before.duration<0.||before.volume>100||!matches!(before.state.as_str(),"play"|"pause"|"stop"|"idle"){return Err("EQ_CURSOR_UNCONFIRMED");}
        let mut state=before.clone();
        state.state=if state.state=="play"{"pause".into()}else{state.state};
        state.audio.clear(); // No negotiated format or rendered frames yet.
        Ok(Self{state})
    }
    pub fn observe(&self,actual:Playback)->Result<Playback>{
        if !matches!(actual.state.as_str(),"stop"|"idle"){return Err("EQ_CURSOR_UNCONFIRMED");}
        let mut state=self.state.clone();state.volume=actual.volume;Ok(state)
    }
    pub fn position(&self)->f64{self.state.elapsed}
    pub fn paused(&self)->bool{self.state.state=="pause"}
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state(kind:&str)->Playback{Playback{state:kind.into(),elapsed:83.125,duration:200.,volume:27,audio:"96000:24:2".into()}}
    #[test]
    fn failed_stop_acknowledgements_do_not_publish_a_new_zero_cursor_or_retry(){
        let before=state("pause");
        for fail in ["stop","clearerror"]{
            let mut log=vec![];
            assert!(Staged::stop_after(Some(&before),|c|{log.push(c.to_string());if c==fail{Err("own_ack_failure")}else{Ok(())}}).is_err());
            assert_eq!(before.elapsed,83.125);assert_eq!(before.state,"pause");
            assert_eq!(log,if fail=="stop"{vec!["stop"]}else{vec!["stop","clearerror"]});
        }
    }
    #[test]
    fn stop_commits_a_valid_cursor_only_after_both_acks_and_preserves_unstaged_command_order(){
        let mut log=vec![];let stopped=Staged::stop_after(Some(&state("pause")),|c|{log.push(c.to_string());Ok(())}).unwrap().unwrap();
        assert_eq!(stopped.position(),0.);assert!(!stopped.paused());assert_eq!(log,vec!["stop","clearerror"]);
        log.clear();assert!(Staged::stop_after(None,|c|{log.push(c.to_string());Ok(())}).unwrap().is_none());assert_eq!(log,vec!["stop","clearerror"]);
        let mut invalid=state("pause");invalid.elapsed=f64::NAN;
        assert!(Staged::stop_after(Some(&invalid),|_|panic!("invalid prior cursor must not contact MPD")).is_err());
    }
    #[test]
    fn paused_switch_keeps_logical_cursor_without_claiming_playback_or_format(){
        for prior in ["play","pause","stop"]{
            let prepared=Staged::new(&state(prior)).unwrap();
            let mut native=state("stop");native.elapsed=0.;native.duration=0.;native.audio.clear();native.volume=30;
            let observed=prepared.observe(native).unwrap();
            assert_eq!(observed.state,if prior=="play"{"pause"}else{prior});
            assert_eq!(observed.elapsed,83.125);assert_eq!(observed.volume,30);assert!(observed.audio.is_empty());
            assert_eq!(prepared.position(),83.125);
        }
    }
    #[test]
    fn foreign_play_or_pause_cannot_be_covered_by_a_logical_stopped_candidate(){
        let p=Staged::new(&state("pause")).unwrap();
        for actual in ["play","pause"]{assert!(p.observe(state(actual)).is_err());}
        let mut invalid=state("pause");invalid.elapsed=f64::NAN;assert!(Staged::new(&invalid).is_err());
        invalid.elapsed=-0.5;assert!(Staged::new(&invalid).is_err());
    }
}
