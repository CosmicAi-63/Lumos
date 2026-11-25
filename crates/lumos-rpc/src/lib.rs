//! JSON-RPC 2.0 over newline-delimited JSON on a Unix socket.
//!
//! The server knows nothing about recording: it hands each request to a
//! [`Dispatch`] implementation (the engine) and forwards engine events to
//! connections that subscribed to them.

pub mod client;
pub mod paths;
pub mod server;
pub mod wire;

pub use client::Client;
pub use server::{ConnCtx, Dispatch, bind, serve};

use std::path::Path;

/// macOS limits `sun_path` to 104 bytes including the terminator.
pub const MAX_SOCKET_PATH: usize = 103;

pub fn check_socket_path(path: &Path) -> Result<(), lumos_model::ToolError> {
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        return Err(lumos_model::ToolError::invalid_params(format!(
            "socket path is {len} bytes; the limit is {MAX_SOCKET_PATH}. Set LUMOS_HOME to a shorter folder: {}",
            path.display()
        )));
    }
    Ok(())
}
