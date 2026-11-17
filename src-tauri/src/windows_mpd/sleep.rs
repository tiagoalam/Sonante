//! STOP confirmation without queue mutation; failure closes only an owned engine.
use super::protocol::{self, Result};
pub trait Route {
    fn command(&mut self, text: &str) -> Result<Vec<String>>;
    fn abort_owned(&mut self) -> Result<()>;
}
#[derive(Debug, PartialEq)]
pub struct Failure {
    pub code: &'static str,
    pub terminated: bool,
}
/// A terminal PCM generation cannot acknowledge ordinary Stop followed by
/// Status. Capture its logical queue before closing both exact owned Jobs.
/// Even a failed observation must never prevent the requested shutdown.
pub fn close_terminal<T,S>(route:&mut T,capture:impl FnOnce(&mut T)->Result<S>,abort:impl FnOnce(&mut T)->Result<()>)->std::result::Result<S,Failure>{
    let saved=capture(route);
    abort(route).map_err(|_|Failure{code:"sleep_stop_cleanup_failed",terminated:false})?;
    saved.map_err(|code|Failure{code,terminated:true})
}
pub fn stop(route: &mut impl Route) -> std::result::Result<(), Failure> {
    let result = (|| {
        route.command("stop")?;
        let observed = protocol::playback(&route.command("status")?)?;
        if observed.state != "stop" {
            return Err("sleep_stop_unconfirmed");
        }
        Ok(())
    })();
    result.map_err(|code| match route.abort_owned() {
        Ok(()) => Failure {
            code,
            terminated: true,
        },
        Err(_) => Failure {
            code: "sleep_stop_cleanup_failed",
            terminated: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        commands: Vec<String>,
        state: &'static str,
        failure: Option<&'static str>,
        aborted: usize,
        abort_fails: bool,
    }
    impl Route for Fake {
        fn command(&mut self, text: &str) -> Result<Vec<String>> {
            self.commands.push(text.into());
            if let Some(code) = self.failure {
                return Err(code);
            }
            Ok(if text == "status" {
                vec![format!("state: {}", self.state), "volume: 50".into()]
            } else {
                vec![]
            })
        }
        fn abort_owned(&mut self) -> Result<()> {
            self.aborted += 1;
            if self.abort_fails {
                Err("timeout")
            } else {
                Ok(())
            }
        }
    }
    fn fake(state: &'static str) -> Fake {
        Fake {
            commands: vec![],
            state,
            failure: None,
            aborted: 0,
            abort_fails: false,
        }
    }
    #[test]
    fn terminal_pcm_closes_after_checkpoint_without_native_stop_status_or_new_engine() {
        let mut route=fake("play");
        let saved=close_terminal(&mut route,|r|{r.commands.push("capture-logical-queue".into());Ok((3usize,28.5))},|r|r.abort_owned()).unwrap();
        assert_eq!(saved,(3,28.5));assert_eq!(route.commands,["capture-logical-queue"]);assert_eq!(route.aborted,1);
    }
    #[test]
    fn failed_terminal_observation_still_closes_owned_jobs_and_preserves_last_confirmed_queue() {
        let mut route=fake("play");
        let result=close_terminal(&mut route,|_|Err::<(),_>("fixture_capture_failed"),|r|r.abort_owned());
        assert_eq!(result,Err(Failure{code:"fixture_capture_failed",terminated:true}));assert_eq!(route.aborted,1);assert!(route.commands.is_empty());
    }
    #[test]
    fn terminal_cleanup_failure_cannot_publish_a_successful_stop_or_new_checkpoint() {
        let mut route=fake("play");route.abort_fails=true;
        let result=close_terminal(&mut route,|_|Ok("new checkpoint"),|r|r.abort_owned());
        assert_eq!(result,Err(Failure{code:"sleep_stop_cleanup_failed",terminated:false}));assert_eq!(route.aborted,1);
    }
    #[test]
    fn stop_never_toggles_rebuilds_or_clears_and_requires_confirmation() {
        let mut route = fake("stop");
        assert_eq!(stop(&mut route), Ok(()));
        assert_eq!(route.commands, ["stop", "status"]);
        assert_eq!(route.aborted, 0);
        for state in ["play", "pause", "invalid"] {
            let mut route = fake(state);
            assert!(stop(&mut route).unwrap_err().terminated);
            assert_eq!(route.commands, ["stop", "status"]);
            assert_eq!(route.aborted, 1);
        }
    }
    #[test]
    fn ack_timeout_eof_uncertain_or_device_failure_abort_only_owned_engine_without_replay() {
        for code in [
            "mpd_ack",
            "mpd_timeout",
            "mpd_eof",
            "PLAYBACK_QUEUE_UNCERTAIN",
            "device_unavailable",
        ] {
            let mut route = fake("play");
            route.failure = Some(code);
            assert_eq!(
                stop(&mut route),
                Err(Failure {
                    code,
                    terminated: true
                })
            );
            assert_eq!(route.commands, ["stop"]);
            assert_eq!(route.aborted, 1);
        }
    }
    #[test]
    fn failed_owned_cleanup_is_explicit_and_cannot_confirm_silence() {
        let mut route = fake("pause");
        route.failure = Some("mpd_timeout");
        route.abort_fails = true;
        assert_eq!(
            stop(&mut route),
            Err(Failure {
                code: "sleep_stop_cleanup_failed",
                terminated: false
            })
        );
        assert_eq!(route.aborted, 1);
    }
}
