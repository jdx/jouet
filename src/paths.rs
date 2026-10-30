//! Where jouet keeps files that outlive a command, such as full logs.

use std::path::PathBuf;

/// `$JOUET_LOG_DIR`, else `<state dir>/logs`.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = env_dir("JOUET_LOG_DIR") {
        return dir;
    }
    state_dir().join("logs")
}

/// `$XDG_STATE_HOME/jouet`, else `~/.local/state/jouet`, else a directory
/// under the system temp dir.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", &[".local", "state"])
}

/// `$XDG_CACHE_HOME/jouet`, else `~/.cache/jouet`.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", &[".cache"])
}

/// `$XDG_CONFIG_HOME/jouet`, else `~/.config/jouet`: `config.toml` and the
/// user's `rules/`.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", &[".config"])
}

fn xdg(var: &str, fallback: &[&str]) -> PathBuf {
    let base = env_dir(var).or_else(|| {
        std::env::home_dir().map(|home| fallback.iter().fold(home, |p, part| p.join(part)))
    });
    match base {
        Some(base) => base.join("jouet"),
        None => std::env::temp_dir().join("jouet"),
    }
}

pub fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}
