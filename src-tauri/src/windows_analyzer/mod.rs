//! Adapter to the original presentation lease and event schema.
mod collector;
mod contract;
use crate::windows_player::Owner;
use crate::windows_shared::analyzer::Frame;
use serde::Serialize;
use tauri::{Emitter, Manager};
#[derive(Clone)]
pub(crate) struct Native(collector::Collector);
#[derive(Clone, Serialize)]
struct Envelope<T: Clone + Serialize> {
    owner: Owner,
    data: T,
}
#[derive(Clone, Serialize)]
struct Status {
    available: bool,
    reason: Option<&'static str>,
}
fn current(app: &tauri::AppHandle, owner: &Owner) -> bool {
    app.get_webview_window(&owner.host).is_some()
        && app
            .state::<crate::windows_player::State>()
            .is_current(owner)
}
impl Native {
    pub(crate) fn new(controller: &super::Controller) -> Self {
        Self(collector::Collector::new(controller.analyzer_view()))
    }
    pub(crate) fn start(&self, app: tauri::AppHandle, owner: Owner) -> Result<(), String> {
        let scope_app = app.clone();
        let scope_owner = owner.clone();
        let status_app = app.clone();
        let status_owner = owner.clone();
        let frame_app = app;
        let frame_owner = owner.clone();
        let target = collector::Target {
            owner,
            current: std::sync::Arc::new(move || current(&scope_app, &scope_owner)),
            status: std::sync::Arc::new(move |available| {
                if current(&status_app, &status_owner) {
                    let _ = status_app.emit_to(
                        &status_owner.host,
                        crate::windows_player::STATUS_EVENT,
                        Envelope {
                            owner: status_owner.clone(),
                            data: Status {
                                available,
                                reason: if available {
                                    None
                                } else {
                                    Some("windows_pending")
                                },
                            },
                        },
                    );
                }
            }),
            frame: std::sync::Arc::new(move |data: Frame| {
                if current(&frame_app, &frame_owner) {
                    let _ = frame_app.emit_to(
                        &frame_owner.host,
                        "now-playing://audio-level",
                        Envelope {
                            owner: frame_owner.clone(),
                            data,
                        },
                    );
                }
            }),
        };
        if self.0.start(target) {
            Ok(())
        } else {
            Err("windows_player_host_unavailable".into())
        }
    }
    pub(crate) fn claimed(&self, generation: u64) {
        self.0.claimed(generation);
    }
    pub(crate) fn stopped(&self, host: &str, session: &str, generation: u64, lease: &str) {
        self.0.stopped(host, session, generation, lease);
    }
    pub(crate) fn released(&self, session: &str, generation: u64) {
        self.0.released(session, generation);
    }
    pub(crate) fn closed(&self, host: &str) {
        self.0.closed(host);
    }
    pub(crate) fn shutdown(&self) {
        self.0.shutdown();
    }
}
