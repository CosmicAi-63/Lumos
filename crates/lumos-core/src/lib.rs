//! The engine: recorder state machine, settings, jobs and finalize.
//!
//! Every client (UI, CLI, MCP) reaches it only through [`Engine`]'s
//! `lumos_rpc::Dispatch` implementation, using the tool names in `lumos-tools`.

mod engine;
mod finalize;
mod fsutil;
mod jobs;
mod policy;
mod recorder;
mod settings;

pub use engine::{Engine, EngineOptions};
pub use fsutil::expand_tilde;
