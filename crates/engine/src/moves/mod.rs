//! Moving a chat — its run, workspace and agent session — to another of the
//! account's engines (docs/session-move.md).
//!
//! The source engine copies the workspace while the agent keeps working,
//! stops the agent between steps, sends the last changes and the agent's
//! own session, and the target takes the chat over. See `service.rs` for
//! the shape, `source.rs` and `target.rs` for each end.

pub mod git;
mod harvest;
mod lineage;
mod note;
pub mod protocol;
pub mod scope;
mod scout;
mod service;
mod source;
mod target;

pub use service::{MoveService, MoveServiceConfig, Tickets};
