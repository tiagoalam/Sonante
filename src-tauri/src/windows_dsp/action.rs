//! Admission for actions that invalidate a PCM generation. Other actions stay
//! on the original Windows actor path (pause, gain, options, metadata edits).
use crate::windows_mpd::{protocol::{Playback,Result},action::Action,options::PlaybackOptions};
pub(crate) struct Plan{pub index:usize,pub elapsed:f64,pub play:bool,pub stop:bool}
pub(crate) fn plan(action:&Action,state:&Playback,index:usize,len:usize,radio:bool,options:Option<PlaybackOptions>,started:bool)->Result<Option<Plan>>{
    let make=|index,elapsed,play,stop|Ok(Some(Plan{index,elapsed,play,stop}));
    match action {
        Action::Toggle if radio&&state.state=="play"=>make(index,0.,false,true),
        Action::SetPlaying{want_play:false} if radio=>make(index,0.,false,true),
        Action::Stop=>make(index,0.,false,true),
        Action::Seek{seconds,index:expected}=>{
            if radio{return Err("RADIO_LIVE_NOT_SEEKABLE");}
            if (len>1&&expected!=&Some(index))||expected.is_some_and(|i|i!=index){return Err("stale_request");}
            if !seconds.is_finite()||*seconds<0.||state.duration<=0.||*seconds>state.duration||!matches!(state.state.as_str(),"play"|"pause"){return Err("invalid_seek");}
            make(index,*seconds,state.state=="play",false)
        }
        Action::Jump{index}=>{if *index>=len{return Err("queue_boundary");}make(*index,0.,true,false)},
        Action::Next|Action::Previous=>{
            if len==0||radio{return Err("queue_boundary");}
            // A fresh MPD process does not export its random play-order history.
            // Never silently turn Previous into ordinal order or invent a
            // random target. Requires an owned order snapshot/restore adapter.
            if options.is_some_and(|o|o.random){return Err("EQ_RANDOM_STEP_UNAVAILABLE");}
            // Native MPD handle_next temporarily clears single: repeat-one
            // governs natural completion, never an explicit Next gesture.
            // Native Previous at the first non-repeat track restarts it.
            use crate::windows_mpd::navigation::{ordinal,Direction};
            let selected=ordinal(if matches!(action,Action::Next){Direction::Next}else{Direction::Previous},index,len,options.is_some_and(|o|o.repeat))?;
            make(selected.index(),selected.elapsed(),true,false)
        }
        Action::Toggle|Action::SetPlaying{want_play:true} if state.state=="stop"&&started=>make(index,0.,true,false),
        _=>Ok(None),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows_mpd::options::SingleMode;
    fn state(kind:&str)->Playback{Playback{state:kind.into(),elapsed:20.,duration:200.,volume:50,audio:"44100:16:2".into()}}
    fn options(random:bool,repeat:bool,single:SingleMode)->PlaybackOptions{PlaybackOptions{random,repeat,single}}
    #[test]
    fn paused_seek_replaces_generation_without_autoplay_and_rejects_stale_or_live_cursors(){
        for kind in ["pause","play"]{
            let p=plan(&Action::Seek{seconds:80.,index:Some(1)},&state(kind),1,3,false,None,true).unwrap().unwrap();
            assert_eq!(p.index,1);assert_eq!(p.elapsed,80.);assert_eq!(p.play,kind=="play");assert!(!p.stop);
        }
        for (seconds,index,radio) in [(80.,Some(0),false),(f64::NAN,Some(1),false),(300.,Some(1),false),(80.,Some(1),true)]{
            assert!(plan(&Action::Seek{seconds,index},&state("pause"),1,3,radio,None,true).is_err());
        }
    }
    #[test]
    fn stop_never_autoplays_but_explicit_restart_after_eof_creates_a_fresh_generation(){
        let p=plan(&Action::Stop,&state("play"),1,3,false,None,true).unwrap().unwrap();
        assert!(!p.play&&p.stop);assert_eq!(p.elapsed,0.);
        let p=plan(&Action::Toggle,&state("stop"),1,3,false,None,true).unwrap().unwrap();
        assert!(p.play);assert_eq!(p.elapsed,0.);
        assert!(plan(&Action::Toggle,&state("pause"),1,3,false,None,true).unwrap().is_none());
        assert!(plan(&Action::Volume{value:30},&state("play"),1,3,false,None,true).unwrap().is_none());
        let radio=plan(&Action::Toggle,&state("play"),0,1,true,None,true).unwrap().unwrap();assert!(radio.stop&&!radio.play);assert_eq!(radio.elapsed,0.);
    }
    #[test]
    fn repeat_single_and_boundaries_follow_the_existing_mpd_contract_without_guessing_random_order(){
        let p=plan(&Action::Next,&state("play"),1,3,false,Some(options(false,true,SingleMode::On)),true).unwrap().unwrap();assert_eq!(p.index,2);
        let p=plan(&Action::Next,&state("play"),2,3,false,Some(options(false,true,SingleMode::Off)),true).unwrap().unwrap();assert_eq!(p.index,0);
        let p=plan(&Action::Previous,&state("play"),0,3,false,Some(options(false,true,SingleMode::Off)),true).unwrap().unwrap();assert_eq!(p.index,2);
        assert!(plan(&Action::Next,&state("play"),2,3,false,None,true).is_err());
        assert!(matches!(plan(&Action::Previous,&state("play"),1,3,false,Some(options(true,false,SingleMode::Off)),true),Err("EQ_RANDOM_STEP_UNAVAILABLE")));
    }
    #[test]
    fn explicit_next_ignores_single_while_retaining_repeat_wrap_and_original_options(){
        for kind in ["play","pause","stop"]{for single in [SingleMode::Off,SingleMode::On,SingleMode::Oneshot]{for repeat in [false,true]{
            let original=options(false,repeat,single);
            let next=plan(&Action::Next,&state(kind),1,3,false,Some(original),true).unwrap().unwrap();
            assert_eq!(next.index,2);assert_eq!(next.elapsed,0.);assert!(next.play&&!next.stop);
            let last=plan(&Action::Next,&state(kind),2,3,false,Some(original),true);
            if repeat{assert_eq!(last.unwrap().unwrap().index,0);}else{assert!(matches!(last,Err("queue_boundary")));}
            assert_eq!(original,options(false,repeat,single));
        }}}
    }
    #[test]
    fn explicit_previous_restarts_first_without_repeat_and_wraps_only_with_repeat(){
        for kind in ["play","pause","stop"]{for single in [SingleMode::Off,SingleMode::On,SingleMode::Oneshot]{
            for repeat in [false,true]{
                let previous=plan(&Action::Previous,&state(kind),0,3,false,Some(options(false,repeat,single)),true).unwrap().unwrap();
                assert_eq!(previous.index,if repeat{2}else{0});assert_eq!(previous.elapsed,0.);assert!(previous.play&&!previous.stop);
                let middle=plan(&Action::Previous,&state(kind),1,3,false,Some(options(false,repeat,single)),true).unwrap().unwrap();assert_eq!(middle.index,0);
            }
        }}
        assert_eq!(plan(&Action::Previous,&state("play"),0,1,false,None,true).unwrap().unwrap().index,0);
        assert!(matches!(plan(&Action::Previous,&state("play"),0,0,false,None,true),Err("queue_boundary")));
    }
}
