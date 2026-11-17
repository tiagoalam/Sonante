//! Confirm the primary process has exited AND its private Job has no members.
//! No PID lookup, unrelated process or job-limit policy changes.
use std::time::{Duration,Instant};
#[derive(Clone,Copy)]
pub(crate) struct Observation{pub main_dead:bool,pub active:u32}
pub(crate) fn wait(mut query:impl FnMut()->Result<Observation,&'static str>,timeout:Duration)->Result<(),&'static str>{
    let end=Instant::now()+timeout;
    loop{
        let observed=query()?;
        if observed.main_dead&&observed.active==0{return Ok(());}
        let Some(remaining)=end.checked_duration_since(Instant::now())else{return Err("job_shutdown_unconfirmed")};
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}
#[cfg(windows)]
pub(crate) fn owned(process:windows_sys::Win32::Foundation::HANDLE,job:windows_sys::Win32::Foundation::HANDLE,timeout:Duration)->Result<(),&'static str>{
    use windows_sys::Win32::{Foundation::{WAIT_OBJECT_0,WAIT_TIMEOUT},System::{Threading::WaitForSingleObject,JobObjects::{QueryInformationJobObject,JobObjectBasicAccountingInformation,JOBOBJECT_BASIC_ACCOUNTING_INFORMATION}}};
    wait(||{
        let state=unsafe{WaitForSingleObject(process,0)};
        if !matches!(state,WAIT_OBJECT_0|WAIT_TIMEOUT){return Err("job_state_unconfirmed");}
        let mut info:JOBOBJECT_BASIC_ACCOUNTING_INFORMATION=unsafe{std::mem::zeroed()};
        if unsafe{QueryInformationJobObject(job,JobObjectBasicAccountingInformation,(&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),std::mem::size_of_val(&info)as u32,std::ptr::null_mut())}==0{return Err("job_state_unconfirmed");}
        Ok(Observation{main_dead:state==WAIT_OBJECT_0,active:info.ActiveProcesses})
    },timeout)
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn dead_primary_with_a_live_member_does_not_acknowledge_shutdown(){assert_eq!(wait(||Ok(Observation{main_dead:true,active:1}),Duration::ZERO),Err("job_shutdown_unconfirmed"));}
    #[test]fn empty_job_without_primary_death_is_insufficient(){assert_eq!(wait(||Ok(Observation{main_dead:false,active:0}),Duration::ZERO),Err("job_shutdown_unconfirmed"));}
    #[test]fn already_quiescent_scope_needs_no_sleep_or_extra_termination(){let mut calls=0;assert_eq!(wait(||{calls+=1;Ok(Observation{main_dead:true,active:0})},Duration::ZERO),Ok(()));assert_eq!(calls,1);}
    #[test]fn retain_scope_until_its_remaining_member_exits(){let mut states=[Observation{main_dead:false,active:2},Observation{main_dead:true,active:1},Observation{main_dead:true,active:0}].into_iter();assert_eq!(wait(||Ok(states.next().unwrap()),Duration::from_secs(1)),Ok(()));assert!(states.next().is_none());}
    #[test]fn query_failure_cannot_mean_an_empty_job(){assert_eq!(wait(||Err("job_state_unconfirmed"),Duration::from_secs(1)),Err("job_state_unconfirmed"));}
}
