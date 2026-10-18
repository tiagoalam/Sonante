use super::*;
use crate::config_save_operation::{BackgroundAdmission, Coordinator};
use std::sync::mpsc;

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "sonante-watch-updates-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
fn watcher<F: Fn(Batch) -> Reconcile + Send + 'static>(
    state: &LocalWatcherState,
    context: Context,
    callback: F,
    notice: mpsc::Sender<&'static str>,
) {
    let w = LocalWatcher::start_context(
        context,
        state.mailbox.clone(),
        Duration::from_millis(25),
        Duration::from_millis(5),
        Duration::from_millis(50),
        Duration::from_secs(2),
        state.shutdown.clone(),
        callback,
        move |n| {
            notice.send(n).ok();
        },
    )
    .unwrap();
    assert!(w.wait_until_ready(Duration::from_secs(2)));
    *state.watcher.lock().unwrap() = Some(w);
}
fn receive<T>(r: &mpsc::Receiver<T>) -> T {
    r.recv_timeout(Duration::from_secs(4)).unwrap()
}

#[test]
fn save_busy_retains_coalesced_batch_until_release_without_second_event() {
    let r = root();
    let state = LocalWatcherState::new();
    let coordinator = Arc::new(Coordinator::new());
    let (_, op) = coordinator.reserve().unwrap();
    let c = coordinator.clone();
    let (attempt, attempts) = mpsc::channel();
    let (success, successes) = mpsc::channel();
    let (notice, _) = mpsc::channel();
    watcher(
        &state,
        Context::for_roots(&[r.clone()]),
        move |batch| match c.try_begin_background() {
            Err(BackgroundAdmission::Busy) => {
                attempt.send(()).unwrap();
                Reconcile::Busy
            }
            Err(BackgroundAdmission::Blocked) => Reconcile::Blocked,
            Ok(_guard) => {
                success.send(batch.paths).unwrap();
                Reconcile::Confirmed
            }
        },
        notice,
    );
    for i in 0..8 {
        fs::write(r.join(format!("{i}.flac")), b"fixture").unwrap();
    }
    receive(&attempts);
    op.finish(Ok(()), false);
    let paths = receive(&successes);
    assert_eq!(paths.len(), 8);
    // The delivered batch was cleared; no second event/attempt is needed.
    assert!(successes.recv_timeout(Duration::from_millis(80)).is_err());
    state.begin_shutdown();
    state.stop_and_join();
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn confirmed_handoff_observes_new_root_interval_then_removal_empty_and_refill() {
    let r = root();
    let a = r.join("A");
    let b = r.join("B");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let state = LocalWatcherState::new();
    let (tx, rx) = mpsc::channel();
    let (nt, nr) = mpsc::channel();
    watcher(
        &state,
        Context::for_roots(&[a.clone()]),
        move |batch| {
            tx.send(batch.paths).unwrap();
            Reconcile::Confirmed
        },
        nt,
    );
    assert_eq!(receive(&nr), "ready");
    let prepared = state.prepare(vec![a.clone(), b.clone()]).unwrap();
    // Events arrive BEFORE commit/handoff; no draft callback may observe B.
    fs::write(b.join("during-save.flac"), b"closed").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(70)).is_err());
    let ab = Context::for_roots(&[a.clone(), b.clone()]);
    crate::after_confirmed_local_folders_save(Ok(()), false, || {
        state.install(ab.clone(), Some(prepared)).unwrap()
    })
    .unwrap();
    assert_eq!(receive(&nr), "ready");
    assert!(receive(&rx).contains(&b.join("during-save.flac")));
    let mut only_b = ab.clone();
    only_b.sources.retain(|_, s| s.root == b);
    state
        .install(only_b, Some(state.prepare(vec![b.clone()]).unwrap()))
        .unwrap();
    assert_eq!(receive(&nr), "ready");
    fs::write(a.join("removed.flac"), b"ignored").unwrap();
    fs::write(b.join("survivor.flac"), b"included").unwrap();
    let paths = receive(&rx);
    assert!(paths.iter().all(|p| p.starts_with(&b)));
    let empty = Context::for_roots(&[]);
    state
        .install(empty, Some(state.prepare(vec![]).unwrap()))
        .unwrap();
    assert_eq!(receive(&nr), "ready");
    fs::write(b.join("not-watched.flac"), b"fixture").unwrap();
    state
        .install(
            Context::for_roots(&[b.clone()]),
            Some(state.prepare(vec![b.clone()]).unwrap()),
        )
        .unwrap();
    assert_eq!(receive(&nr), "ready");
    fs::write(b.join("refill.flac"), b"fixture").unwrap();
    assert!(receive(&rx).contains(&b.join("refill.flac")));
    state.begin_shutdown();
    state.stop_and_join();
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn draft_observer_dropped_on_failure_leaves_effective_roots_unchanged() {
    let r = root();
    let a = r.join("A");
    let b = r.join("B");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let state = LocalWatcherState::new();
    let (tx, rx) = mpsc::channel();
    let (nt, _) = mpsc::channel();
    watcher(
        &state,
        Context::for_roots(&[a.clone()]),
        move |batch| {
            tx.send(batch.paths).unwrap();
            Reconcile::Confirmed
        },
        nt,
    );
    for uncertain in [false, true] {
        let prepared = state.prepare(vec![a.clone(), b.clone()]).unwrap();
        let result: Result<(), String> = Err("Save failed/AfterRenameUnconfirmed".into());
        assert!(
            crate::after_confirmed_local_folders_save(result, uncertain, || state
                .install(Context::for_roots(&[a.clone(), b.clone()]), Some(prepared))
                .unwrap())
            .is_err()
        );
    }
    fs::write(b.join("draft-only.flac"), b"fixture").unwrap();
    fs::write(a.join("control.flac"), b"fixture").unwrap();
    assert!(receive(&rx).iter().all(|p| p.starts_with(&a)));
    state.begin_shutdown();
    state.stop_and_join();
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn handoff_preserves_open_writes_and_never_retries_uncertain_or_shutdown() {
    let r = root();
    let state = LocalWatcherState::new();
    let (tx, rx) = mpsc::channel();
    let (nt, nr) = mpsc::channel();
    watcher(
        &state,
        Context::for_roots(&[r.clone()]),
        move |batch| {
            tx.send(batch.paths).unwrap();
            Reconcile::Confirmed
        },
        nt,
    );
    assert_eq!(receive(&nr), "ready");
    let prepared = state.prepare(vec![r.clone()]).unwrap();
    let mut open = File::create(r.join("open.flac")).unwrap();
    use std::io::Write;
    open.write_all(b"not finished").unwrap();
    state
        .install(Context::for_roots(&[r.clone()]), Some(prepared))
        .unwrap();
    assert_eq!(receive(&nr), "ready");
    assert!(rx.recv_timeout(Duration::from_millis(90)).is_err());
    drop(open);
    assert!(receive(&rx).contains(&r.join("open.flac")));
    let prepared = state.prepare(vec![r.clone()]).unwrap();
    state.begin_shutdown();
    assert!(state
        .install(Context::for_roots(&[r.clone()]), Some(prepared))
        .is_err());
    state.stop_and_join();
    assert!(state.prepare(vec![r.clone()]).is_err());
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn stale_batch_is_rebased_only_for_surviving_reserved_source_and_bounded() {
    let r = root();
    let a = r.join("A");
    let b = r.join("B");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let old = Context::for_roots(&[a.clone(), b.clone()]);
    let mut next = old.clone();
    next.sources.retain(|_, s| s.root == b);
    let now = Instant::now();
    let mut pending = PendingChanges::default();
    pending.mark_path(a.join("a.flac"), now);
    pending.mark_path(b.join("b.flac"), now);
    pending.rebase(&old, &next);
    assert_eq!(pending.paths, BTreeSet::from([b.join("b.flac")]));
    for i in 0..=MAX_DIRTY_PATHS {
        pending.mark_path(b.join(format!("{i}.flac")), now);
    }
    assert!(pending.overflowed);
    assert_eq!(pending.paths.len(), MAX_DIRTY_PATHS);
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn blocked_worker_pauses_without_spinning_and_recovers_only_with_prepared_observer() {
    let r = root();
    let state = LocalWatcherState::new();
    let (tx, rx) = mpsc::channel();
    let (nt, nr) = mpsc::channel();
    let blocked = Arc::new(AtomicBool::new(true));
    let blocked_copy = blocked.clone();
    watcher(
        &state,
        Context::for_roots(&[r.clone()]),
        move |_| {
            tx.send(()).unwrap();
            if blocked_copy.load(Ordering::SeqCst) {
                Reconcile::Blocked
            } else {
                Reconcile::Confirmed
            }
        },
        nt,
    );
    assert_eq!(receive(&nr), "ready");
    fs::write(r.join("first.flac"), b"fixture").unwrap();
    receive(&rx);
    assert_eq!(receive(&nr), "reconciliation_blocked");
    state
        .install(Context::for_roots(&[r.clone()]), None)
        .unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(70)).is_err());
    blocked.store(false, Ordering::SeqCst);
    state
        .install(
            Context::for_roots(&[r.clone()]),
            Some(state.prepare(vec![r.clone()]).unwrap()),
        )
        .unwrap();
    assert_eq!(receive(&nr), "ready");
    fs::write(r.join("after-manual-rescan.flac"), b"fixture").unwrap();
    receive(&rx);
    state.begin_shutdown();
    state.stop_and_join();
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn open_write_renamed_inside_source_remains_pending_until_close() {
    let r = root();
    let state = LocalWatcherState::new();
    let (tx, rx) = mpsc::channel();
    let (nt, _) = mpsc::channel();
    watcher(
        &state,
        Context::for_roots(&[r.clone()]),
        move |batch| {
            tx.send(batch.paths).unwrap();
            Reconcile::Confirmed
        },
        nt,
    );
    let temporary = r.join("writing.tmp");
    let target = r.join("final.flac");
    let mut file = File::create(&temporary).unwrap();
    use std::io::Write;
    file.write_all(b"partial").unwrap();
    fs::rename(&temporary, &target).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    drop(file);
    assert!(receive(&rx).contains(&target));
    state.begin_shutdown();
    state.stop_and_join();
    fs::remove_dir_all(r).unwrap();
}

#[test]
fn prepared_aliases_deduplicate_and_second_queued_handoff_is_refused_before_effects() {
    let r = root();
    let alias = r.with_extension("alias");
    std::os::unix::fs::symlink(&r, &alias).unwrap();
    let state = LocalWatcherState::new();
    let prepared = state.prepare(vec![r.clone(), alias.clone()]).unwrap();
    assert_eq!(prepared.sources.len(), 1);
    state
        .install(Context::for_roots(&[r.clone()]), Some(prepared))
        .unwrap();
    assert!(state
        .prepare(vec![r.clone()])
        .err()
        .unwrap()
        .contains("HANDOFF_PENDING"));
    state.begin_shutdown();
    state.stop_and_join();
    fs::remove_file(alias).unwrap();
    fs::remove_dir_all(r).unwrap();
}
