//! Socket location shared by the daemon and its clients.

use std::env;
use std::path::PathBuf;

/// Environment variable that overrides the socket location.
pub const SOCKET_ENV: &str = "OSAIMA_MCP_SOCKET";

/// Resolve the daemon socket path.
///
/// Order: `$OSAIMA_MCP_SOCKET`, then `$XDG_RUNTIME_DIR/osaima/mcp.sock`, then
/// `/tmp/osaima-<uid>/mcp.sock`.
pub fn socket_path() -> PathBuf {
    if let Some(path) = env::var_os(SOCKET_ENV).filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(dir).join("osaima").join("mcp.sock");
    }
    PathBuf::from(format!("/tmp/osaima-{}", current_uid())).join("mcp.sock")
}

/// Effective user id of this process.
pub fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}
