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

/// Where jouet's hooks keep their ledger: `$XDG_STATE_HOME/jouet/ledger`,
/// else `~/.local/state/jouet/ledger`.
pub fn ledger_dir() -> PathBuf {
    state_dir().join("ledger")
}

/// Where `jouet stats` keeps what it worked out, such as the baseline from
/// before setup: `$XDG_STATE_HOME/jouet/stats`.
pub fn stats_dir() -> PathBuf {
    state_dir().join("stats")
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
