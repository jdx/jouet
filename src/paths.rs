//! Where jouet keeps files that outlive a command, such as full logs.
//!
//! Only logs fall back to the system temp directory when there is no home
//! directory. Config, cache and state hold rules, compiled bytecode and trust
//! decisions, which another user could plant in a shared temp directory, so
//! without a home they are simply absent.

use std::path::PathBuf;

/// `$JOUET_LOG_DIR`, else `<state dir>/logs`, else a directory under the
/// system temp dir.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = env_dir("JOUET_LOG_DIR") {
        return dir;
    }
    state_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("jouet"))
        .join("logs")
}

/// `$XDG_STATE_HOME/jouet`, else `~/.local/state/jouet`.
pub fn state_dir() -> Option<PathBuf> {
    xdg("XDG_STATE_HOME", &[".local", "state"])
}

/// `$XDG_CACHE_HOME/jouet`, else `~/.cache/jouet`.
pub fn cache_dir() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", &[".cache"])
}

/// `$XDG_CONFIG_HOME/jouet`, else `~/.config/jouet`: `config.toml` and the
/// user's `rules/`.
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", &[".config"])
}

fn xdg(var: &str, fallback: &[&str]) -> Option<PathBuf> {
    let base = env_dir(var).filter(|d| d.is_absolute()).or_else(|| {
        std::env::home_dir().map(|home| fallback.iter().fold(home, |p, part| p.join(part)))
    })?;
    Some(base.join("jouet"))
}

pub fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}
