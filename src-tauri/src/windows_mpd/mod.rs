mod admission;
mod observation;
mod mute;
pub(crate) mod process;
pub(crate) mod job_scope;
mod stdio;
mod media_server;
#[path="radio_transport.rs"]
mod radio;
pub mod protocol;
pub mod session;
pub(crate) mod transaction;

pub mod library;

mod queue;
mod queue_finish;
pub mod options;
pub mod queue_edit;

pub mod sleep;

pub mod source;

// Test-only access for the startup admission versus real journal regression.
#[cfg(test)]
pub(crate) use queue::{Journal as TestJournal, SavedQueue as TestSavedQueue};

// Stable wire representation shared by the live Windows journal and source adapters.
pub(crate) mod queue_v3;
pub(crate) mod queue_v4;

pub(crate) mod transport;

mod config_transaction;

#[cfg(test)]
pub(crate) use media_server::Server as TestMediaServer;
pub(crate) mod action;
pub(crate) mod navigation;

pub(crate) mod liveness;
