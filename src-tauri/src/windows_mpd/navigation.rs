//! Manual navigation admission. A logical paused cursor is not a playing MPD
//! or its random order. Only explicit resume may consume that old cursor.
use super::{options::PlaybackOptions,protocol::Result};
pub(crate) const ORDER_UNAVAILABLE:&str="PLAYBACK_RANDOM_ORDER_UNAVAILABLE";
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Direction{Next,Previous}
impl Direction{pub(crate) fn command(self)->&'static str{match self{Self::Next=>"next",Self::Previous=>"previous"}}}
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct Selected{index:usize,elapsed:f64}
impl Selected{
    pub(crate) fn index(self)->usize{self.index}
    pub(crate) fn elapsed(self)->f64{self.elapsed}
}
#[derive(Debug,PartialEq)]
pub(crate) enum Step{Native(Direction),Selected(Selected)}
pub(crate) fn jump(index:usize,len:usize)->Result<Selected>{
    if len==0||index>=len{return Err("queue_boundary");}Ok(Selected{index,elapsed:0.})
}
pub(crate) fn ordinal(direction:Direction,index:usize,len:usize,repeat:bool)->Result<Selected>{
    if len==0||index>=len{return Err("queue_boundary");}
    let target=match direction{
        Direction::Next=>index.checked_add(1).filter(|i|*i<len).or_else(||repeat.then_some(0)),
        Direction::Previous=>index.checked_sub(1).or_else(||Some(if repeat{len-1}else{0})),
    }.ok_or("queue_boundary")?;
    jump(target,len)
}
pub(crate) fn step(direction:Direction,index:usize,len:usize,state:&str,staged:bool,options:Option<PlaybackOptions>)->Result<Step>{
    if !matches!(state,"play"|"pause"|"stop"|"idle"){return Err("queue_unconfirmed");}
    if len==0||index>=len{return Err("queue_boundary");}
    if !staged&&matches!(state,"play"|"pause")&&options.is_some_and(|o|o.random||o.repeat){return Ok(Step::Native(direction));}
    if options.is_some_and(|o|o.random){return Err(ORDER_UNAVAILABLE);}
    ordinal(direction,index,len,options.is_some_and(|o|o.repeat)).map(Step::Selected)
}
#[cfg(test)]mod tests{
    use super::*;use super::super::options::SingleMode;
    fn options(random:bool,repeat:bool,single:SingleMode)->PlaybackOptions{PlaybackOptions{random,repeat,single}}
    #[test]fn restored_or_staged_navigation_selects_zero_position_and_never_a_native_command(){
        for state in["play","pause","stop","idle"]{for single in[SingleMode::Off,SingleMode::On,SingleMode::Oneshot]{for repeat in[false,true]{
            for direction in[Direction::Next,Direction::Previous]{
                let Step::Selected(selected)=step(direction,1,3,state,true,Some(options(false,repeat,single))).unwrap()else{panic!("logical cursor cannot grant native play state")};
                assert_eq!(selected.index(),if direction==Direction::Next{2}else{0});assert_eq!(selected.elapsed(),0.);
            }
        }}}
        for index in 0..3{assert_eq!(jump(index,3).unwrap().elapsed(),0.);assert_eq!(jump(index,3).unwrap().index(),index);}
        assert!(jump(3,3).is_err());assert!(jump(0,0).is_err());
    }
    #[test]fn only_a_real_live_mpd_retains_authority_over_repeat_or_random_history(){
        for state in["play","pause"]{for direction in[Direction::Next,Direction::Previous]{for(random,repeat)in[(true,false),(true,true),(false,true)]{
            assert_eq!(step(direction,1,3,state,false,Some(options(random,repeat,SingleMode::On))).unwrap(),Step::Native(direction));
        }}}
        for state in["stop","idle"]{for direction in[Direction::Next,Direction::Previous]{
            assert!(matches!(step(direction,1,3,state,false,Some(options(true,true,SingleMode::Off))),Err(ORDER_UNAVAILABLE)));
        }}
        for state in["play","pause","stop","idle"]{assert!(matches!(step(Direction::Next,1,3,state,true,Some(options(true,false,SingleMode::Off))),Err(ORDER_UNAVAILABLE)));}
    }
    #[test]fn explicit_ordinals_preserve_first_previous_and_repeat_wrap_independently_of_single(){
        assert_eq!(ordinal(Direction::Previous,0,3,false).unwrap().index(),0);assert_eq!(ordinal(Direction::Previous,0,3,true).unwrap().index(),2);
        assert_eq!(ordinal(Direction::Next,2,3,true).unwrap().index(),0);assert!(ordinal(Direction::Next,2,3,false).is_err());
        for direction in[Direction::Next,Direction::Previous]{assert!(ordinal(direction,0,0,true).is_err());assert!(ordinal(direction,3,3,true).is_err());assert!(ordinal(direction,usize::MAX,3,true).is_err());}
        assert!(step(Direction::Next,0,3,"unconfirmed",false,None).is_err());
    }
}
