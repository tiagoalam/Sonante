//! Optional catalog work has typed cancellation: it is not an indexing failure.
use crate::{
    audio::{AudioEngine, MpdRefreshCompletion},
    local_media::mirror::ManagedRefreshPlan,
};
use std::time::Duration;

pub(crate) const CANCELLED: &str = "LOCAL_RECONCILIATION_CANCELLED";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed,
    Cancelled,
}

pub(crate) fn refresh(
    socket: &str,
    plan: &ManagedRefreshPlan,
    audit: bool,
    shutdown: impl FnMut() -> bool,
) -> Result<Outcome, String> {
    refresh_runtime(socket, plan, audit, shutdown, false)
}

pub(crate) fn refresh_filesystem(socket: &str, plan: &ManagedRefreshPlan, shutdown: impl FnMut() -> bool) -> Result<Outcome, String> {
    refresh_runtime(socket, plan, true, shutdown, true)
}

fn refresh_runtime(socket: &str, plan: &ManagedRefreshPlan, audit: bool, shutdown: impl FnMut() -> bool, force_paths: bool) -> Result<Outcome, String> {
    let started = std::time::Instant::now();
    let result = refresh_with(
        plan,
        audit,
        shutdown,
        |directory| match directory {
            None => AudioEngine::request_mpd_rescan_job(socket),
            Some(path) if force_paths => AudioEngine::rescan_library_path(socket, path),
            Some(path) => AudioEngine::update_library_path(socket, path)
                .map_err(|error| format!("Falha ao solicitar update direcionado: {error:?}")),
        },
        |job, shutdown| {
            AudioEngine::wait_for_managed_mpd_update(
                socket,
                Some(job),
                Duration::from_millis(100),
                shutdown,
            )
        },
        || {
            let result = AudioEngine::audit_mpd_local_catalog_at(socket)?;
            if !result.is_clean() {
                return Err(format!(
                    "A auditoria do catálogo encontrou {} URI(s) local(is) inválida(s).",
                    result.invalid_count
                ));
            }
            Ok(())
        },
    );
    let strategy = match plan {
        ManagedRefreshPlan::MetadataOnly => "metadata",
        ManagedRefreshPlan::Full => "full",
        ManagedRefreshPlan::Targeted { .. } => "targeted",
    };
    let outcome = match &result {
        Ok(Outcome::Completed) => "completed",
        Ok(Outcome::Cancelled) => "cancelled",
        Err(_) => "error",
    };
    eprintln!(
        "[Mirror] managed_refresh={strategy} elapsed_ms={} outcome={outcome}",
        started.elapsed().as_millis()
    );
    result
}

fn refresh_with(
    plan: &ManagedRefreshPlan,
    audit: bool,
    mut shutdown: impl FnMut() -> bool,
    mut request: impl FnMut(Option<&str>) -> Result<u64, String>,
    mut wait: impl FnMut(u64, &mut dyn FnMut() -> bool) -> Result<MpdRefreshCompletion, ()>,
    mut inspect: impl FnMut() -> Result<(), String>,
) -> Result<Outcome, String> {
    // A confirmed metadata-only publication has no pending MPD work. Finishing
    // its bookkeeping during shutdown must not invent an unaudited index job.
    // Publication independently rechecks that only namespace/schema changed.
    if matches!(plan, ManagedRefreshPlan::MetadataOnly) {
        return Ok(Outcome::Completed);
    }
    if shutdown() {
        return Ok(Outcome::Cancelled);
    }
    let directories = match plan {
        ManagedRefreshPlan::MetadataOnly => unreachable!("handled above"),
        ManagedRefreshPlan::Full => vec![None],
        ManagedRefreshPlan::Targeted { directories, .. } => {
            directories.iter().map(|p| Some(p.as_str())).collect()
        }
    };
    for directory in directories {
        if shutdown() {
            return Ok(Outcome::Cancelled);
        }
        let job = request(directory);
        if shutdown() {
            return Ok(Outcome::Cancelled);
        }
        let completion = wait(job?, &mut shutdown);
        // A shutdown observed during bounded I/O wins over recovery by error.
        if shutdown() || completion.is_err() {
            return Ok(Outcome::Cancelled);
        }
        let completion = completion.expect("checked cancellation above");
        if completion != MpdRefreshCompletion::Completed {
            return Err(format!(
                "Falha durante o refresh da biblioteca: {}",
                crate::describe_refresh_failure(completion)
            ));
        }
    }
    if shutdown() {
        return Ok(Outcome::Cancelled);
    }
    if audit {
        let result = inspect();
        if shutdown() {
            return Ok(Outcome::Cancelled);
        }
        result?;
    }
    Ok(Outcome::Completed)
}

/// Preserve both generations when cancellation occurs after publication. The
/// existing startup staging guard then prevents treating an unaudited DB as
/// confirmed. The coordinated shutdown owns stopping the process, after the
/// transaction/restart permit is released. No recovery restart is initiated.
pub(crate) fn finish(
    result: Result<Outcome, String>,
    mut complete_or_recover: impl FnMut(Result<(), String>) -> Result<(), String>,
) -> Result<(), String> {
    match result {
        Ok(Outcome::Cancelled) => Err(format!("{CANCELLED}: atualização cancelada no encerramento; gerações preservadas, catálogo não confirmado. Revisão necessária antes de nova abertura.")),
        Ok(Outcome::Completed) => complete_or_recover(Ok(())),
        Err(error) => complete_or_recover(Err(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        sync::{mpsc, Arc},
        thread,
    };
    #[test]
    fn confirmed_metadata_only_needs_no_mpd_io_even_during_shutdown() {
        for shutdown in [false, true] {
            let result = refresh_with(
                &ManagedRefreshPlan::MetadataOnly,
                true,
                || shutdown,
                |_| panic!("no request"),
                |_, _| panic!("no wait"),
                || panic!("no audit"),
            );
            assert_eq!(result.unwrap(), Outcome::Completed);
        }
    }
    #[test]
    fn targeted_requests_keep_order_and_audit_only_after_completed_jobs() {
        use std::cell::RefCell;
        let trace = RefCell::new(Vec::new());
        let jobs = Cell::new(0);
        let plan = ManagedRefreshPlan::Targeted {
            directories: vec!["Music/One".into(), "Music/Two".into()],
            changed_files: 2,
        };
        assert_eq!(
            refresh_with(
                &plan,
                true,
                || false,
                |path| {
                    trace.borrow_mut().push(path.unwrap().to_string());
                    jobs.set(jobs.get() + 1);
                    Ok(jobs.get())
                },
                |job, _| {
                    trace.borrow_mut().push(format!("wait-{job}"));
                    Ok(MpdRefreshCompletion::Completed)
                },
                || {
                    trace.borrow_mut().push("audit".into());
                    Ok(())
                }
            )
            .unwrap(),
            Outcome::Completed
        );
        assert_eq!(
            *trace.borrow(),
            ["Music/One", "wait-1", "Music/Two", "wait-2", "audit"]
        );
    }
    #[test]
    fn explicit_failures_still_require_recovery_and_no_early_audit() {
        for completion in [
            MpdRefreshCompletion::TransportError("fixture".into()),
            MpdRefreshCompletion::ProtocolError("fixture".into()),
            MpdRefreshCompletion::Unavailable("fixture".into()),
        ] {
            let result = refresh_with(
                &ManagedRefreshPlan::Full,
                true,
                || false,
                |p| {
                    assert!(p.is_none());
                    Ok(42)
                },
                |_, _| Ok(completion.clone()),
                || panic!("no audit after failure"),
            );
            let called = Cell::new(false);
            let error = finish(result, |result| {
                called.set(true);
                result
            })
            .unwrap_err();
            assert!(called.get());
            assert!(!error.contains(CANCELLED));
        }
    }
    #[test]
    fn shutdown_during_request_wait_or_audit_has_no_recovery_or_success() {
        for point in ["before", "request", "wait", "audit"] {
            let closing = Cell::new(point == "before");
            let audit_calls = Cell::new(0);
            let result = refresh_with(
                &ManagedRefreshPlan::Full,
                true,
                || closing.get(),
                |_| {
                    closing.set(point == "request");
                    Ok(42)
                },
                |_, shutdown| {
                    closing.set(point == "wait");
                    if shutdown() {
                        Err(())
                    } else {
                        Ok(MpdRefreshCompletion::Completed)
                    }
                },
                || {
                    audit_calls.set(audit_calls.get() + 1);
                    closing.set(true);
                    Ok(())
                },
            );
            assert_eq!(result.unwrap(), Outcome::Cancelled);
            assert!(finish(Ok(Outcome::Cancelled), |_| panic!(
                "cancel must not stop/rollback/restart/cleanup/commit"
            ))
            .unwrap_err()
            .starts_with(CANCELLED));
            assert_eq!(audit_calls.get(), u32::from(point == "audit"));
        }
    }
    #[test]
    fn transaction_and_restart_ownership_release_after_cancellation_without_blocking_notification()
    {
        let state = Arc::new(crate::ConfigTransactionState::new());
        let transaction = state.begin().unwrap();
        let restart = state.try_begin_restart().unwrap();
        let other = Arc::clone(&state);
        let (requested_tx, requested_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            other.request_shutdown();
            requested_tx.send(()).unwrap();
            other.begin_effective_shutdown();
            let _transaction = other.wait_for_active_transaction();
            done_tx.send(()).unwrap();
        });
        requested_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            refresh_with(
                &ManagedRefreshPlan::Full,
                true,
                || state.shutdown_requested(),
                |_| panic!("no request after shutdown"),
                |_, _| panic!("no wait"),
                || panic!("no audit")
            )
            .unwrap(),
            Outcome::Cancelled
        );
        assert!(done_rx.try_recv().is_err());
        drop(restart);
        drop(transaction);
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(state.try_begin_restart().is_none());
    }
    #[test]
    fn cancelled_started_recovery_keeps_generations_without_cleanup_or_claimed_restore() {
        let trace = std::cell::RefCell::new(Vec::new());
        let error = crate::complete_published_mirror_refresh(
            || Err("real refresh failure".into()),
            || Ok(()),
            || {
                trace.borrow_mut().push("stop-before-recovery");
                Ok(())
            },
            || {
                trace.borrow_mut().push("rollback-before-recovery");
                Ok(())
            },
            |_| Ok(crate::RestorePreviousOutcome::CancelledDuringRecovery),
            || panic!("uncertain generations must remain"),
            || panic!("no commit"),
            || panic!("no commit cleanup"),
        )
        .unwrap_err();
        assert!(error.contains(CANCELLED));
        assert!(error.contains("não confirmados"));
        assert!(!error.contains("reprodução restaurada"));
        assert_eq!(
            *trace.borrow(),
            ["stop-before-recovery", "rollback-before-recovery"]
        );
    }
    #[test]
    fn actual_refresh_requests_then_waits_then_audits_using_only_fake_mpd() {
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 42\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Album/track.flac\nOK\n"),
        ]);
        assert_eq!(
            refresh(&socket, &ManagedRefreshPlan::Full, true, || false).unwrap(),
            Outcome::Completed
        );
        server.join().unwrap();
    }
}
