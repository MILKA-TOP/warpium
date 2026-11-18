//! An MCP server (stdio, newline-delimited JSON-RPC 2.0) that lets coding agents read an Android
//! app's device logs while they work on it: recent lines, crashes with their stack frames resolved
//! to project files, and waiting for a line after restarting the app.
//!
//! Logs may hold secrets or personal data, so queries are scoped to one app package unless the
//! caller explicitly asks for every package, output is capped, and nothing is written to disk:
//! each device's lines live in a bounded in-memory ring while the server runs.

mod capture;
pub mod crash;
pub mod server;
pub mod since;

pub use server::{Server, ServerOptions};
