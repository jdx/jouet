//! Where jouet keeps files that outlive a command, such as full logs.

use std::path::PathBuf;

/// `$JOUET_LOG_DIR`, else `$XDG_STATE_HOME/jouet/logs`, else
/// `~/.local/state/jouet/logs`, else a directory under the system temp dir.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JOUET_LOG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".local").join("state")));
    match state {
        Some(state) => state.join("jouet").join("logs"),
        None => std::env::temp_dir().join("jouet").join("logs"),
    }
}
