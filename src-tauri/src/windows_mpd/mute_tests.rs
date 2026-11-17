use super::Memory;
#[test]
fn hosts_share_idempotent_state_and_actual_volume_wins() {
    let mut worker = Memory::default();
    assert_eq!(worker.plan(3, 3, "windows:3", 37, true), Ok(Some(0))); // main
    assert_eq!(worker.plan(3, 3, "windows:3", 0, true), Ok(None)); // detached duplicate
    assert_eq!(worker.plan(3, 3, "windows:3", 0, false), Ok(Some(37))); // detached unmute
    assert_eq!(worker.plan(3, 3, "windows:3", 25, false), Ok(None)); // slider changed actual level
    assert_eq!(worker.plan(3, 3, "windows:3", 25, true), Ok(Some(0)));
    assert_eq!(worker.plan(3, 3, "windows:3", 0, false), Ok(Some(25)));
}
#[test]
fn stale_token_or_new_generation_never_restores_old_context_or_a_default() {
    let mut worker = Memory::default();
    assert_eq!(worker.plan(3, 3, "windows:3", 61, true), Ok(Some(0)));
    assert_eq!(
        worker.plan(4, 4, "windows:3", 0, false),
        Err("SHARED_MUTE_CONTEXT_CHANGED")
    );
    assert_eq!(
        worker.plan(4, 4, "windows:4", 0, false),
        Err("SHARED_MUTE_LEVEL_UNKNOWN")
    );
    assert_eq!(
        worker.plan(4, 4, "windows:4", 101, true),
        Err("SHARED_MUTE_CONTEXT_CHANGED")
    );
    assert_eq!(worker.plan(4, 4, "windows:4", 0, true), Ok(None));
    assert_eq!(
        Memory::default().plan(4, 4, "windows:4", 0, false),
        Err("SHARED_MUTE_LEVEL_UNKNOWN")
    );
}
#[test]
fn failed_engine_write_does_not_invent_mute_or_retry_implicitly() {
    let mut worker = Memory::default();
    assert_eq!(worker.plan(8, 8, "windows:8", 17, true), Ok(Some(0)));
    // The owner reports failure; next fresh observation still says 17.
    assert_eq!(worker.plan(8, 8, "windows:8", 17, false), Ok(None));
    assert_eq!(worker.plan(8, 8, "windows:8", 17, true), Ok(Some(0)));
    assert_eq!(worker.plan(8, 8, "windows:8", 0, false), Ok(Some(17)));
}
#[test]
fn exhaustive_software_adapter_matches_unchanged_linux_policy() {
    use crate::shared_mute::{Action, Memory as Original};
    for first in 0..=101u8 {
        for requested in [false, true] {
            let mut original = Original::default();
            let mut adapted = Memory::default();
            for (generation, token, value, want) in [
                (1, "windows:1", first, requested),
                (1, "windows:1", 0, !requested),
                (1, "windows:1", 25, false),
                (2, "windows:1", 0, false),
                (2, "windows:2", 0, false),
                (2, "windows:2", 99, true),
                (2, "windows:2", 0, false),
            ] {
                let expected = original
                    .plan(
                        token,
                        &format!("windows:{generation}"),
                        value.into(),
                        value == 0,
                        false,
                        want,
                    )
                    .map(|v| match v {
                        Action::None => None,
                        Action::Volume(v) => Some(v as u8),
                        Action::Mute(_) => panic!("software route"),
                    });
                let got = adapted
                    .plan(generation, generation, token, value, want)
                    .map_err(str::to_owned);
                assert_eq!(got, expected);
            }
        }
    }
}

#[test]
fn queue_revisions_do_not_replace_volume_owner_but_engine_changes_do() {
    let mut worker = Memory::default();
    assert_eq!(worker.plan(10, 1, "windows:10", 37, true), Ok(Some(0)));
    // Insertion/reordering/removal advances queue generation, same owned MPD.
    assert_eq!(
        worker.plan(11, 1, "windows:10", 0, false),
        Err("SHARED_MUTE_CONTEXT_CHANGED")
    );
    assert_eq!(worker.plan(11, 1, "windows:11", 0, false), Ok(Some(37)));
    assert_eq!(worker.plan(12, 1, "windows:12", 0, false), Ok(Some(37)));
    // Owned engine replaced/recovered, or logical queue explicitly cleared.
    assert_eq!(
        worker.plan(13, 2, "windows:13", 0, false),
        Err("SHARED_MUTE_LEVEL_UNKNOWN")
    );
    assert_eq!(worker.plan(13, 2, "windows:13", 25, true), Ok(Some(0)));
    assert_eq!(worker.plan(14, 2, "windows:14", 0, false), Ok(Some(25)));
}
