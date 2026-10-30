//! Where jouet keeps files that outlive a command, such as full logs.

use std::path::PathBuf;

/// `$JOUET_LOG_DIR`, else `$XDG_STATE_HOME/jouet/logs`, else
/// `~/.local/state/jouet/logs`, else a directory under the system temp dir.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JOUET_LOG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    state_dir().join("logs")
}

/// Where `wait` keeps the shared state of the PRs being watched:
/// `$XDG_STATE_HOME/jouet/watch`, else `~/.local/state/jouet/watch`.
pub fn watch_dir() -> PathBuf {
    state_dir().join("watch")
}

/// Where `jouet setup`'s hooks keep each agent session's state, such as its
/// last test and lint runs: `$XDG_STATE_HOME/jouet/sessions`, else
/// `~/.local/state/jouet/sessions`.
pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

fn state_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".local").join("state")));
    match state {
        Some(state) => state.join("jouet"),
        None => std::env::temp_dir().join("jouet"),
    }
}
