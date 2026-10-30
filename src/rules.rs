//! Loading jouet's rules: built-in, the user's, and a trusted repository's.

use jouet_rules::{Budget, Engine, Options, Trust, TrustStore};
use std::path::{Path, PathBuf};

pub struct Loaded {
    pub engine: Engine,
    /// Rule files that could not be read.
    pub errors: Vec<String>,
    /// This repository's rules directory and whether it is trusted.
    pub repo: Option<(PathBuf, Trust)>,
}

/// `~/.config/jouet/rules`, when there is a home directory.
pub fn user_dir() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("rules"))
}

/// `<repository root>/.jouet/rules`, when there is one above `cwd`.
pub fn repo_dir(cwd: &Path) -> Option<PathBuf> {
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let dir = root.join(".jouet").join("rules");
    dir.is_dir().then_some(dir)
}

pub fn trust_store() -> TrustStore {
    TrustStore::load(crate::paths::state_dir().map(|d| d.join("trusted-rules.json")))
}

/// Loads every rule. Fails only when the Luau VM itself cannot start.
pub fn load(cwd: &Path) -> Result<Loaded, String> {
    let trust = trust_store();
    let repo = repo_dir(cwd);
    let (sources, errors) = jouet_rules::discover(user_dir().as_deref(), repo.as_deref(), &trust);
    let settings = crate::settings::Settings::load();
    let engine = Engine::new(
        sources,
        Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: settings.rules.disable.unwrap_or_default(),
            budget: Budget::default(),
            cache_dir: crate::paths::cache_dir().map(|d| d.join("luau")),
        },
    )
    .map_err(|e| e.to_string())?;
    let repo = repo.map(|dir| {
        let state = trust.check(&dir);
        (dir, state)
    });
    Ok(Loaded {
        engine,
        errors,
        repo,
    })
}
