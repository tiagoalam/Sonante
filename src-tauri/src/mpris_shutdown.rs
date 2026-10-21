//! A local name-release acknowledgement must not hold audio cleanup indefinitely.
use std::{future::Future, time::Duration};

// Local bus bookkeeping has no network/user-work dependency. One second leaves
// scheduling margin but bounds this optional acknowledgement before MPD/DSP join.
pub(crate) const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) async fn bounded_release<F: Future>(
    release: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(RELEASE_TIMEOUT, release).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    fn run<T>(f: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }
    #[test]
    fn normal_release_and_error_preserve_the_result() {
        assert_eq!(
            run(bounded_release(async { Ok::<_, &'static str>(true) })).unwrap(),
            Ok(true)
        );
        assert_eq!(
            run(bounded_release(async { Err::<bool, _>("own bus failed") })).unwrap(),
            Err("own bus failed")
        );
    }
    #[test]
    fn delayed_normal_reply_is_awaited() {
        let start = std::time::Instant::now();
        assert!(run(bounded_release(async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            true
        }))
        .unwrap());
        assert!(start.elapsed() >= Duration::from_millis(30));
    }
    #[test]
    fn timeout_drops_the_pending_future_before_following_cleanup() {
        struct OwnedFuture(Arc<AtomicBool>);
        impl Drop for OwnedFuture {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let start = std::time::Instant::now();
        assert!(run(bounded_release(async move {
            let _owned = OwnedFuture(flag);
            std::future::pending::<()>().await
        }))
        .is_err());
        assert!(dropped.load(Ordering::SeqCst));
        assert!(
            start.elapsed() >= RELEASE_TIMEOUT
                && start.elapsed() < RELEASE_TIMEOUT + Duration::from_secs(2)
        );
        // No detached task is spawned by the deadline wrapper.
    }
    #[test]
    fn release_timeout_or_bus_failure_does_not_skip_owned_child_cleanup() {
        use crate::supervisor::MpdSupervisor;
        use std::process::Command;
        for failed_helper in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "sonante-release-cleanup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            let mut child = if failed_helper {
                Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap()
            } else {
                Command::new("sleep").arg("30").spawn().unwrap()
            };
            let pid = child.id();
            if failed_helper {
                assert!(!child.wait().unwrap().success());
            }
            let mut supervisor = MpdSupervisor::new_with_runtime_paths(
                &root.join("mpd.socket"),
                &root.join("mpd.pid"),
            );
            supervisor.set_process_for_test(child, true);
            if failed_helper {
                assert!(
                    run(bounded_release(async { Err::<(), _>("own bus failed") }))
                        .unwrap()
                        .is_err()
                );
            } else {
                assert!(run(bounded_release(std::future::pending::<()>())).is_err());
            }
            supervisor.stop().unwrap();
            supervisor.stop().unwrap();
            assert!(!supervisor.has_process_for_test());
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
