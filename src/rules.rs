//! Loading jouet's rules: built-in, the user's, and a trusted repository's.

use jouet_rules::{Budget, Engine, Options, Probes, Trust, TrustStore};
use std::path::{Path, PathBuf};

pub(crate) struct Loaded {
    pub engine: Engine,
    /// Rule files that could not be read.
    pub errors: Vec<String>,
    /// This repository's rules directory and whether it is trusted.
    pub repo: Option<(PathBuf, Trust)>,
}

/// `~/.config/jouet/rules`, when there is a home directory.
pub(crate) fn user_dir() -> Option<PathBuf> {
    jouet_output::paths::config_dir().map(|d| d.join("rules"))
}

/// `<repository root>/.jouet/rules`, when there is one above `cwd`.
pub(crate) fn repo_dir(cwd: &Path) -> Option<PathBuf> {
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let dir = root.join(".jouet").join("rules");
    dir.is_dir().then_some(dir)
}

/// `~/.config/jouet/recipes`, when there is a home directory.
pub(crate) fn user_recipe_dir() -> Option<PathBuf> {
    jouet_output::paths::config_dir().map(|d| d.join("recipes"))
}

/// `<repository root>/.jouet/recipes`, when there is one above `cwd`.
pub(crate) fn repo_recipe_dir(cwd: &Path) -> Option<PathBuf> {
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let dir = root.join(".jouet").join("recipes");
    dir.is_dir().then_some(dir)
}

/// What the probes read for a rule or recipe running in `cwd`: git there,
/// and the hooks' ledger. `network` lets the GitHub probes run.
pub(crate) fn probes(cwd: &Path, network: bool) -> Probes {
    let ledger = jouet_output::paths::state_dir().map(|d| d.join("ledger"));
    Probes::new(Some(cwd.to_path_buf()), ledger, network)
        .with_repo_key(crate::ledger::repo_key(cwd))
}

pub(crate) fn trust_store() -> TrustStore {
    TrustStore::load(jouet_output::paths::state_dir().map(|d| d.join("trusted-rules.json")))
}

/// Loads every rule. A script's Luau VM starts only when a call needs it.
pub(crate) fn load(cwd: &Path) -> Loaded {
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
            cache_dir: jouet_output::paths::cache_dir().map(|d| d.join("luau")),
        },
    );
    // In a hook the GitHub probes wait for `rules.network`.
    engine.set_probes(probes(cwd, settings.rules.network));
    let repo = repo.map(|dir| {
        let state = trust.check(&dir);
        (dir, state)
    });
    Loaded {
        engine,
        errors,
        repo,
    }
}
