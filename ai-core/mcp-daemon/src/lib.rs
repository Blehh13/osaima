//! OSAIMA AI Core — a Model Context Protocol (MCP) server that exposes live
//! system context and safe system actions to the shell and to AI agents.
//!
//! The daemon speaks newline-delimited JSON-RPC 2.0 over a per-user Unix
//! socket (see [`paths::socket_path`]) or over stdio (`--stdio`), so standard
//! MCP clients can launch it directly.

pub mod paths;
pub mod protocol;
pub mod server;
pub mod system;
pub mod tools;
pub mod transport;
