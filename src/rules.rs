//! Loading jouet's rules: built-in, and those of every set
//! [`crate::sources`] finds.

use crate::sources::{self, Kind, Set};
use jouet_rules::{Budget, Engine, Options, Origin, Probes, RuleDir};
use std::path::{Path, PathBuf};

pub(crate) struct Loaded {
    pub engine: Engine,
    /// Rule files that could not be read.
    pub errors: Vec<String>,
    /// Every set of rules and routines found, and whether it runs.
    pub sets: Vec<Set>,
}

/// `~/.config/jouet/rules`, when there is a home directory.
pub(crate) fn user_dir() -> Option<PathBuf> {
    jouet_output::paths::config_dir().map(|d| d.join("rules"))
}

/// What the probes read for a rule running in `cwd`: git there,
/// and the hooks' ledger. `network` lets the GitHub probes run.
pub(crate) fn probes(cwd: &Path, network: bool) -> Probes {
    let ledger = jouet_output::paths::state_dir().map(|d| d.join("ledger"));
    Probes::new(Some(cwd.to_path_buf()), ledger, network)
        .with_repo_key(crate::ledger::repo_key(cwd))
        .with_claims(jouet_output::paths::state_dir().map(|d| d.join("claims")))
        .with_test_adapters(|dir| {
            crate::affected::near(dir)
                .into_iter()
                .map(str::to_string)
                .collect()
        })
}

/// What a set's rules are.
fn origin(set: &Set) -> fn(PathBuf) -> Origin {
    match set.kind {
        _ if set.enforced() => Origin::Enforced,
        Kind::User => Origin::User,
        Kind::Repo => Origin::Repo,
        Kind::Plugin | Kind::System => Origin::Plugin,
    }
}

/// How to load: as `jouet trust` sees a repository before trusting it.
#[derive(Default)]
pub(crate) struct Load<'a> {
    /// Treat the repository with this `.jouet` as trusted.
    pub assume: Option<&'a Path>,
    /// Leave every rule from outside unchecked against its examples; the
    /// caller checks them.
    pub ungated: bool,
}

/// Loads every rule. A script's Luau VM starts only when a call needs it.
pub(crate) fn load(cwd: &Path) -> Loaded {
    load_with(cwd, &Load::default())
}

pub(crate) fn load_with(cwd: &Path, how: &Load) -> Loaded {
    let mut digests = sources::digests();
    let mut errors = Vec::new();
    let sets = sources::sets(cwd, how.assume, &mut digests, &mut errors);
    digests.save();
    let dirs: Vec<RuleDir> = sets
        .iter()
        .filter(|s| s.loads() && s.rules_dir().is_dir())
        .map(|s| RuleDir {
            dir: s.rules_dir(),
            origin: origin(s),
            trusted: s.status.runs(),
            namespace: s.namespace(),
        })
        .collect();
    let (rule_sources, read_errors) = jouet_rules::discover(&dirs);
    errors.extend(read_errors);
    let settings = crate::settings::Settings::load();
    let mut engine = Engine::new(
        rule_sources,
        Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: settings.rules.disable.unwrap_or_default(),
            budget: Budget::default(),
            cache_dir: jouet_output::paths::cache_dir().map(|d| d.join("luau")),
        },
    );
    if !how.ungated {
        gate(&mut engine, &sets);
    }
    // In a hook the GitHub probes wait for `rules.network`.
    engine.set_probes(probes(cwd, settings.rules.network));
    Loaded {
        engine,
        errors,
        sets,
    }
}

/// Turns off each rule from outside, a repository's, a plugin's or the
/// system's, that does not pass its examples. Which ones do not is cached
/// by the set's hash and this jouet's version, so a hook runs them once.
fn gate(engine: &mut Engine, sets: &[Set]) {
    for set in sets
        .iter()
        .filter(|s| s.kind != Kind::User && s.status.runs())
    {
        let Some(digest) = &set.digest else {
            continue;
        };
        let file = proven_file(digest);
        let cached: Option<Vec<(String, String)>> = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let failing = cached.unwrap_or_else(|| {
            let failing = engine.unproven(&set.rules_dir());
            remember(file.as_deref(), &failing);
            failing
        });
        for (name, why) in &failing {
            engine.set_broken(name, &set.rules_dir(), why);
        }
    }
}

fn proven_file(digest: &str) -> Option<PathBuf> {
    jouet_output::paths::cache_dir().map(|d| {
        d.join("proven")
            .join(format!("{}-{digest}.json", env!("CARGO_PKG_VERSION")))
    })
}

/// Records that the rules of the set with this hash pass their examples,
/// so the next hook need not run them; failing to is harmless.
pub(crate) fn remember_proven(digest: &str) {
    remember(proven_file(digest).as_deref(), &[]);
}

fn remember(file: Option<&Path>, failing: &[(String, String)]) {
    if let Some(file) = file
        && let Ok(bytes) = serde_json::to_vec(failing)
    {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, bytes);
    }
}
