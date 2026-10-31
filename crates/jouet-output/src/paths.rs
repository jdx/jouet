//! Where jouet keeps files that outlive a command, such as full logs.
//!
//! Only logs fall back to the system temp directory when there is no home
//! directory. Config, cache and state hold rules, compiled bytecode and trust
//! decisions, which another user could plant in a shared temp directory, so
//! without a home they are simply absent.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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

/// Where `wait` keeps the shared state of the PRs being watched:
/// `<state dir>/watch`, else a directory under the system temp dir.
pub fn watch_dir() -> PathBuf {
    state_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("jouet"))
        .join("watch")
}

pub fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// The work tree containing `cwd`: the nearest directory with a `.git`,
/// with symlinks resolved, which is what `git rev-parse --show-toplevel`
/// prints. Read from the filesystem, because a hook cannot afford to start
/// git.
pub fn work_tree(cwd: &Path) -> Option<PathBuf> {
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    root.canonicalize().ok()
}

/// The work tree containing `cwd`, as the first 16 hex characters of the
/// SHA-256 of its root ([`work_tree`]). The ledger and the code graph key
/// their records by it.
pub fn repo_key(cwd: &Path) -> Option<String> {
    let root = work_tree(cwd)?;
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repo_key_hashes_the_work_tree_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        let key = repo_key(&root.join("src/deep")).unwrap();
        assert_eq!(key.len(), 16);
        assert_eq!(repo_key(&root), Some(key.clone()));
        let expected = Sha256::digest(root.canonicalize().unwrap().to_string_lossy().as_bytes());
        assert!(
            expected
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                .starts_with(&key)
        );
        assert_eq!(repo_key(dir.path()), None);
    }
}
