//! Point-in-time output evidence, separate from configuration and audio control.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

struct Slot<T> {
    revision: u64,
    value: Option<(u64, Arc<T>)>,
}
pub struct View<T>(Mutex<Slot<T>>);
impl<T> Default for View<T> {
    fn default() -> Self {
        Self(Mutex::new(Slot {
            revision: 0,
            value: None,
        }))
    }
}
pub struct Binding<T> {
    pub identity: Arc<T>,
    generation: u64,
    revision: u64,
    intent: u64,
}
impl<T> View<T> {
    pub fn publish(&self, generation: u64, identity: Option<Arc<T>>) {
        let Ok(mut slot) = self.0.lock() else { return };
        let same = match (&slot.value, &identity) {
            (Some((old, proof)), Some(next)) => *old == generation && Arc::ptr_eq(proof, next),
            (None, None) => true,
            _ => false,
        };
        if !same {
            slot.revision = slot.revision.wrapping_add(1);
            slot.value = identity.map(|proof| (generation, proof));
        }
    }
    pub fn clear(&self) {
        self.publish(0, None)
    }
    pub fn capture(&self, expected: u64, intent: u64) -> Option<Binding<T>> {
        let slot = self.0.lock().ok()?;
        let (generation, identity) = slot.value.as_ref()?;
        (*generation == expected).then(|| Binding {
            identity: identity.clone(),
            generation: *generation,
            revision: slot.revision,
            intent,
        })
    }
    pub fn current(&self, binding: &Binding<T>, intent: u64) -> bool {
        if binding.intent != intent {
            return false;
        }
        let Ok(slot) = self.0.lock() else {
            return false;
        };
        slot.revision == binding.revision
            && slot.value.as_ref().is_some_and(|(generation, proof)| {
                *generation == binding.generation && Arc::ptr_eq(proof, &binding.identity)
            })
    }
}
pub fn eligible(running: bool, state: &str) -> bool {
    running && state == "play"
}
// The producer owns WASAPI only without EQ. A missing EQ render identity must
// never fall back to that PCM-only producer and report the wrong process.
pub(crate) fn render_owner<T>(equalized:bool,producer:Option<T>,consumer:Option<T>)->Option<T>{
    if equalized{consumer}else{producer}
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[cfg_attr(target_os="windows",derive(serde::Serialize))]
pub struct EndpointVolume{pub value:u8,pub muted:bool}
pub(crate) fn endpoint_volume(gain_hr:i32,gain:f32,mute_hr:i32,muted:bool)->Result<EndpointVolume,&'static str>{
    if gain_hr!=0||mute_hr!=0||!gain.is_finite()||!(0.0..=1.0).contains(&gain){return Err("unreadable");}
    Ok(EndpointVolume{value:(gain*100.0).round()as u8,muted})
}
pub struct ClearOnDrop<T>(pub Arc<View<T>>);
impl<T> Drop for ClearOnDrop<T> {
    fn drop(&mut self) {
        self.0.clear();
    }
}

static BUSY: AtomicBool = AtomicBool::new(false);
pub struct Permit;
impl Permit {
    pub fn acquire() -> Result<Self, &'static str> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "busy")
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

// Exact S_OK only. windows-rs GetProcessId discards successful nonzero HRESULTs,
// including AUDCLNT_S_NO_SINGLE_PROCESS; those do not prove process ownership.
pub fn owned_session(hr: i32, pid: u32, owned_pid: u32) -> Result<bool, &'static str> {
    if hr < 0 {
        return Err("unreadable");
    }
    if pid == 0 || pid != owned_pid {
        return Ok(false);
    }
    if hr != 0 {
        return Err("ambiguous");
    }
    Ok(true)
}
pub fn unique<T>(items: Vec<T>) -> Result<T, &'static str> {
    if items.len() != 1 {
        return Err(if items.is_empty() {
            "not_observed"
        } else {
            "ambiguous"
        });
    }
    Ok(items.into_iter().next().unwrap())
}
pub fn acquire<T>(
    current: impl Fn() -> bool,
    inspect: impl FnOnce() -> Result<T, &'static str>,
) -> Result<T, &'static str> {
    if !current() {
        return Err("changed");
    }
    let value = inspect()?;
    if !current() {
        return Err("changed");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equalized_output_uses_the_consumer_and_never_falls_back_to_pcm_producer(){
        assert_eq!(render_owner(true,Some(1),Some(2)),Some(2));
        assert_eq!(render_owner(true,Some(1),None),None);
        assert_eq!(render_owner(false,Some(1),Some(2)),Some(1));
        assert_eq!(render_owner(false,None,Some(2)),None);
    }
    #[test]
    fn endpoint_level_requires_both_exact_successes_and_bounded_finite_values(){
        assert_eq!(endpoint_volume(0,0.37,0,true),Ok(EndpointVolume{value:37,muted:true}));
        assert_eq!(endpoint_volume(0,0.0,0,false),Ok(EndpointVolume{value:0,muted:false}));
        assert_eq!(endpoint_volume(0,1.0,0,false),Ok(EndpointVolume{value:100,muted:false}));
        for scalar in [f32::NAN,f32::INFINITY,-0.01,1.01]{assert!(endpoint_volume(0,scalar,0,false).is_err());}
        for hr in [-1,1,0x0889000d]{assert!(endpoint_volume(hr,0.5,0,false).is_err());assert!(endpoint_volume(0,0.5,hr,false).is_err());}
    }
    #[test]
    fn logical_restore_pause_stop_and_idle_cannot_authorize_output_observation() {
        for state in ["pause", "stop", "idle", "play"] {
            assert!(!eligible(false, state));
        }
        for state in ["pause", "stop", "idle"] {
            assert!(!eligible(true, state));
        }
        assert!(eligible(true, "play"));
    }
    #[test]
    fn no_binding_at_startup_and_generation_is_not_authority_without_owned_identity() {
        let view = View::<u32>::default();
        assert!(view.capture(0, 0).is_none());
        view.publish(4, Some(Arc::new(123)));
        assert!(view.capture(3, 0).is_none());
        assert!(view.capture(4, 0).is_some());
    }
    #[test]
    fn intent_replacement_stop_resume_and_new_process_reject_old_results() {
        let view = View::default();
        let proof = Arc::new(123);
        view.publish(4, Some(proof.clone()));
        let b = view.capture(4, 8).unwrap();
        assert!(view.current(&b, 8));
        assert!(!view.current(&b, 9));
        view.clear();
        assert!(!view.current(&b, 8));
        view.publish(4, Some(proof));
        assert!(!view.current(&b, 8));
        let b = view.capture(4, 8).unwrap();
        view.publish(4, Some(Arc::new(123)));
        assert!(!view.current(&b, 8));
    }
    #[test]
    fn ordinary_status_tick_preserves_binding_but_queue_generation_and_worker_drop_invalidate() {
        let view = Arc::new(View::default());
        let proof = Arc::new(123);
        view.publish(4, Some(proof.clone()));
        let b = view.capture(4, 8).unwrap();
        for _ in 0..100 {
            view.publish(4, Some(proof.clone()));
            assert!(view.current(&b, 8));
        }
        view.publish(5, Some(proof));
        assert!(!view.current(&b, 8));
        let b = view.capture(5, 8).unwrap();
        drop(ClearOnDrop(view.clone()));
        assert!(!view.current(&b, 8));
    }
    #[test]
    fn multiprocess_success_and_failure_are_never_treated_as_owned_session() {
        assert_eq!(owned_session(0, 123, 123), Ok(true));
        assert_eq!(owned_session(0, 0, 123), Ok(false));
        assert_eq!(owned_session(0, 124, 123), Ok(false));
        assert_eq!(owned_session(0x0889000d, 123, 123), Err("ambiguous"));
        assert_eq!(owned_session(1, 123, 123), Err("ambiguous"));
        assert_eq!(owned_session(-1, 123, 123), Err("unreadable"));
    }
    #[test]
    fn absence_or_multiple_candidates_is_not_an_endpoint_and_does_not_choose_default() {
        assert_eq!(unique(Vec::<u32>::new()), Err("not_observed"));
        assert_eq!(unique(vec![1, 2]), Err("ambiguous"));
        assert_eq!(unique(vec![1]), Ok(1));
    }
    #[test]
    fn stale_before_inspection_skips_io_and_stale_after_discards_facts() {
        assert_eq!(
            acquire(
                || false,
                || -> Result<u32, &'static str> { panic!("must not inspect") }
            ),
            Err("changed")
        );
        let current = std::cell::Cell::new(true);
        assert_eq!(
            acquire(
                || current.get(),
                || {
                    current.set(false);
                    Ok(123)
                }
            ),
            Err("changed")
        );
        assert_eq!(
            acquire(|| true, || Err::<u32, _>("unreadable")),
            Err("unreadable")
        );
    }
    #[test]
    fn bounded_global_admission_retains_permit_until_owner_finishes() {
        let permit = Permit::acquire().unwrap();
        assert!(matches!(Permit::acquire(), Err("busy")));
        drop(permit);
        assert!(Permit::acquire().is_ok());
    }
}

#[cfg(target_os = "windows")]
mod native;
#[cfg(target_os = "windows")]
pub use native::{observe, Observation};
