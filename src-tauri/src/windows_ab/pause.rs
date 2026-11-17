//! A snapshot must confirm silence of the principal engine before WebAudio may
//! start; the intent/intervention captured BEFORE the original pause stay exact.
pub(crate) fn confirm(
    watch: crate::audio::EqTransitionWatch,
    _engine_running: bool,
    state: &str,
) -> Result<crate::audio::EqTransitionWatch, &'static str> {
    if !matches!(state, "pause" | "stop" | "idle") {
        return Err("AB_PAUSE");
    }
    if !watch.current() {
        return Err("AB_CHANGED");
    }
    Ok(watch)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    };
    fn watch() -> crate::audio::EqTransitionWatch {
        crate::audio::EqTransitionWatch {
            intent: Arc::new(AtomicU64::new(7)),
            intent_value: 7,
            intervention: Arc::new(AtomicU64::new(11)),
            intervention_value: 11,
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }
    #[test]
    fn an_ack_or_unpaused_snapshot_is_never_confirmation() {
        for (running, state) in [(true, "play"), (false, "play"), (false, "unknown")] {
            assert!(matches!(confirm(watch(), running, state), Err("AB_PAUSE")));
        }
        for (running, state) in [
            (false, "idle"),
            (false, "stop"),
            (true, "pause"),
            (true, "stop"),
        ] {
            assert!(confirm(watch(), running, state).is_ok());
        }
    }
    #[test]
    fn later_intent_intervention_or_shutdown_invalidates_even_a_paused_snapshot() {
        for which in 0..3 {
            let w = watch();
            match which {
                0 => {
                    w.intent.fetch_add(1, Ordering::AcqRel);
                }
                1 => {
                    w.intervention.fetch_add(1, Ordering::AcqRel);
                }
                _ => w.stopping.store(true, Ordering::Release),
            }
            assert!(matches!(confirm(w, true, "pause"), Err("AB_CHANGED")));
        }
    }
    #[test]
    fn confirmed_watch_keeps_the_original_admitted_intent_and_intervention() {
        let w = watch();
        let other = w.clone();
        let confirmed = confirm(w, true, "pause").unwrap();
        assert!(confirmed.current());
        other.intent.store(8, Ordering::Release);
        assert!(!confirmed.current());
        other.intent.store(7, Ordering::Release);
        other.intervention.store(12, Ordering::Release);
        assert!(!confirmed.current());
    }
}
