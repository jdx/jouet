//! Loading jouet's rules: built-in, the system's, the user's, a trusted
//! repository's, and the rule packs each of those pins.

pub(crate) mod packs;

use jouet_rules::{Budget, Digests, Engine, Options, Origin, Probes, RuleDir, Trust, TrustStore};
use std::path::{Path, PathBuf};

pub(crate) struct Loaded {
    pub engine: Engine,
    /// Rule files that could not be read.
    pub errors: Vec<String>,
    /// Every set of rules found, and whether it runs.
    pub sets: Vec<Set>,
}

/// Where a set of rules comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The system directory, `/etc/jouet`: the organisation's.
    System,
    User,
    Repo,
    Pack,
}

/// Whether a set of rules runs, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    /// The user's own rules.
    Own,
    /// The system directory's: the organisation's rules.
    Enforced,
    /// The system directory, when its group or others can write it.
    Writable,
    Trusted,
    /// Trusted once, but changed since.
    Changed,
    Untrusted,
    /// A pack whose kept rules hash to its pin.
    Verified,
    /// A pack whose kept rules do not hash to its pin.
    Mismatch,
    /// A pack not fetched yet.
    Missing,
    /// A pin that cannot be followed.
    Invalid(String),
}

impl Status {
    pub(crate) fn runs(&self) -> bool {
        matches!(
            self,
            Self::Own | Self::Enforced | Self::Trusted | Self::Verified
        )
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Enforced => "enforced",
            Self::Writable => "writable",
            Self::Trusted => "trusted",
            Self::Changed => "changed",
            Self::Untrusted => "untrusted",
            Self::Verified => "verified",
            Self::Mismatch => "mismatch",
            Self::Missing => "missing",
            Self::Invalid(_) => "invalid",
        }
    }

    fn of(trust: Trust) -> Self {
        match trust {
            Trust::Trusted => Self::Trusted,
            Trust::Changed => Self::Changed,
            Trust::Untrusted => Self::Untrusted,
        }
    }
}

/// A directory of rules, with the pack pins beside it.
#[derive(Debug, Clone)]
pub(crate) struct Set {
    pub kind: Kind,
    /// A pack's name.
    pub name: Option<String>,
    /// For a pack, the kind of set that pins it.
    pub pinned_by: Option<Kind>,
    /// The rule files: `.jouet/rules`, or where a pack is kept.
    pub dir: PathBuf,
    /// The `packs.toml` beside the rules, for a set that can pin packs.
    pub pins: Option<PathBuf>,
    pub status: Status,
    /// Why the system directory's rules do not run.
    pub why: Option<String>,
    /// The hash of the rules and pins, as trust records it.
    pub digest: Option<String>,
    /// A pack's pin.
    pub pin: Option<packs::Pin>,
}

impl Set {
    /// Whether the set's rules run as enforced: the system's, or a pack it
    /// pins.
    pub(crate) fn enforced(&self) -> bool {
        self.kind == Kind::System || self.pinned_by == Some(Kind::System)
    }

    fn origin(&self) -> fn(PathBuf) -> Origin {
        match self.kind {
            _ if self.enforced() => Origin::Enforced,
            Kind::User => Origin::User,
            Kind::Repo => Origin::Repo,
            Kind::Pack | Kind::System => Origin::Pack,
        }
    }

    /// Whether `path`, a rule's file, is one of this set's.
    pub(crate) fn holds(&self, path: &Path) -> bool {
        path.starts_with(&self.dir)
    }

    /// How the set is named to a person: `.jouet/rules`, `pack acme`.
    pub(crate) fn label(&self) -> String {
        match (self.kind, &self.name) {
            (Kind::Pack, Some(name)) => format!("rule pack {name}"),
            _ => self.dir.display().to_string(),
        }
    }
}

/// `~/.config/jouet/rules`, when there is a home directory.
pub(crate) fn user_dir() -> Option<PathBuf> {
    jouet_output::paths::config_dir().map(|d| d.join("rules"))
}

/// The repository root above `cwd`.
fn repo_root(cwd: &Path) -> Option<&Path> {
    cwd.ancestors().find(|dir| dir.join(".git").exists())
}

/// `<repository root>/.jouet/rules`, when there is a repository above
/// `cwd` with rules or pack pins there.
pub(crate) fn repo_dir(cwd: &Path) -> Option<PathBuf> {
    let jouet = repo_root(cwd)?.join(".jouet");
    let dir = jouet.join("rules");
    (dir.is_dir() || jouet.join("packs.toml").is_file()).then_some(dir)
}

/// `$JOUET_SYSTEM_DIR`, else `/etc/jouet` (`%ProgramData%\jouet` on
/// Windows): the organisation's rules, which the user's settings cannot
/// turn off.
pub(crate) fn system_dir() -> Option<PathBuf> {
    if let Some(dir) = jouet_output::paths::env_dir("JOUET_SYSTEM_DIR") {
        return Some(dir);
    }
    if cfg!(windows) {
        jouet_output::paths::env_dir("ProgramData").map(|d| d.join("jouet"))
    } else {
        Some(PathBuf::from("/etc/jouet"))
    }
}

/// `~/.config/jouet/routines`, when there is a home directory.
pub(crate) fn user_routine_dir() -> Option<PathBuf> {
    jouet_output::paths::config_dir().map(|d| d.join("routines"))
}

/// `<repository root>/.jouet/routines`, when there is one above `cwd`.
pub(crate) fn repo_routine_dir(cwd: &Path) -> Option<PathBuf> {
    let dir = repo_root(cwd)?.join(".jouet").join("routines");
    dir.is_dir().then_some(dir)
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

pub(crate) fn trust_store() -> TrustStore {
    TrustStore::load(jouet_output::paths::state_dir().map(|d| d.join("trusted-rules.json")))
}

/// The hashes of rule sets, cached by the files' stat.
pub(crate) fn digests() -> Digests {
    Digests::load(jouet_output::paths::cache_dir().map(|d| d.join("rule-digests.json")))
}

/// How to load: as `trust` sees a directory before trusting it.
#[derive(Default)]
pub(crate) struct Load<'a> {
    /// Treat the set with these rules as trusted.
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
    let mut digests = digests();
    let mut errors = Vec::new();
    let sets = sets(cwd, how.assume, &mut digests, &mut errors);
    digests.save();
    let dirs: Vec<RuleDir> = sets
        .iter()
        .filter(|s| s.dir.is_dir() && !matches!(s.status, Status::Missing | Status::Invalid(_)))
        .map(|s| RuleDir {
            dir: s.dir.clone(),
            origin: s.origin(),
            trusted: s.status.runs(),
        })
        .collect();
    let (sources, read_errors) = jouet_rules::discover(&dirs);
    errors.extend(read_errors);
    let settings = crate::settings::Settings::load();
    let mut engine = Engine::new(
        sources,
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

/// Every set of rules for `cwd`, in the order their rules run: the
/// system's, then the user's, then the repository's, each followed by the
/// packs it pins. A set that does not run pins nothing.
pub(crate) fn sets(
    cwd: &Path,
    assume: Option<&Path>,
    digests: &mut Digests,
    errors: &mut Vec<String>,
) -> Vec<Set> {
    let store = trust_store();
    let mut out = Vec::new();
    let mut trusted =
        |dir: &Path, pins: &Path, digests: &mut Digests| -> (Status, Option<String>) {
            match digests.of(dir, Some(pins)) {
                Ok(digest) if assume.is_some_and(|a| a == dir) => (Status::Trusted, Some(digest)),
                Ok(digest) => (Status::of(store.check_digest(dir, &digest)), Some(digest)),
                Err(e) => {
                    errors.push(format!("reading {}: {e}", dir.display()));
                    (Status::Untrusted, None)
                }
            }
        };
    if let Some(system) = system_dir() {
        let dir = system.join("rules");
        let pins = system.join("packs.toml");
        if dir.is_dir() || pins.is_file() {
            // An unreadable file shows when its rules load.
            let digest = digests.of(&dir, Some(&pins)).ok();
            let why = writable_by_others(&system, &dir, &pins);
            let status = if why.is_some() {
                Status::Writable
            } else {
                Status::Enforced
            };
            out.push(Set {
                kind: Kind::System,
                name: None,
                pinned_by: None,
                dir,
                pins: Some(pins),
                status,
                why,
                digest,
                pin: None,
            });
        }
    }
    if let Some(config) = jouet_output::paths::config_dir() {
        let pins = config.join("packs.toml");
        out.push(Set {
            kind: Kind::User,
            name: None,
            pinned_by: None,
            dir: config.join("rules"),
            pins: Some(pins),
            status: Status::Own,
            why: None,
            digest: None,
            pin: None,
        });
    }
    if let Some(dir) = repo_dir(cwd) {
        let pins = dir.with_file_name("packs.toml");
        let (status, digest) = trusted(&dir, &pins, digests);
        out.push(Set {
            kind: Kind::Repo,
            name: None,
            pinned_by: None,
            dir,
            pins: Some(pins),
            status,
            why: None,
            digest,
            pin: None,
        });
    }
    let mut all = Vec::new();
    for set in out {
        let packs = match (&set.pins, set.status.runs()) {
            (Some(pins), true) => pack_sets(pins, set.kind, digests, errors),
            _ => Vec::new(),
        };
        all.push(set);
        all.extend(packs);
    }
    all
}

/// The packs a `packs.toml` pins, and whether each is kept as pinned.
fn pack_sets(pins: &Path, by: Kind, digests: &mut Digests, errors: &mut Vec<String>) -> Vec<Set> {
    let pinned = match packs::read(pins) {
        Ok(pinned) => pinned,
        Err(e) => {
            errors.push(e);
            return Vec::new();
        }
    };
    pinned
        .pack
        .into_iter()
        .map(|(name, pin)| {
            let dir = packs::stored(&pin).unwrap_or_default();
            let status = match pin.problem() {
                Some(problem) => Status::Invalid(problem),
                None if !dir.is_dir() => Status::Missing,
                None => match digests.of(&dir, None) {
                    Ok(digest) if digest == pin.digest => Status::Verified,
                    _ => Status::Mismatch,
                },
            };
            Set {
                kind: Kind::Pack,
                name: Some(name),
                pinned_by: Some(by),
                dir,
                pins: None,
                status,
                why: None,
                digest: Some(pin.digest.clone()),
                pin: Some(pin),
            }
        })
        .collect()
}

/// Turns off each rule from outside, a repository's, a pack's or the
/// system's, that does not pass its examples. Which ones do not is cached
/// by the set's hash and this jouet's version, so a hook runs them once.
fn gate(engine: &mut Engine, sets: &[Set]) {
    let cache = jouet_output::paths::cache_dir().map(|d| d.join("proven"));
    for set in sets
        .iter()
        .filter(|s| s.kind != Kind::User && s.status.runs())
    {
        let Some(digest) = &set.digest else {
            continue;
        };
        let file = cache
            .as_ref()
            .map(|d| d.join(format!("{}-{digest}.json", env!("CARGO_PKG_VERSION"))));
        let cached: Option<Vec<(String, String)>> = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let failing = cached.unwrap_or_else(|| {
            let failing = engine.unproven(&set.dir);
            remember(file.as_deref(), &failing);
            failing
        });
        for (name, why) in &failing {
            engine.set_broken(name, &set.dir, why);
        }
    }
}

/// Records which rules of a set do not pass their examples, so the next
/// hook need not run them; failing to is harmless.
pub(crate) fn remember_proven(digest: &str, failing: &[(String, String)]) {
    let file = jouet_output::paths::cache_dir().map(|d| {
        d.join("proven")
            .join(format!("{}-{digest}.json", env!("CARGO_PKG_VERSION")))
    });
    remember(file.as_deref(), failing);
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

/// Why the system directory's rules do not run: the directory, or
/// something in it, can be written by its group or others, so it is not the
/// organisation's alone.
#[cfg(unix)]
fn writable_by_others(system: &Path, dir: &Path, pins: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let mut paths = vec![system.to_path_buf()];
    if dir.is_dir() {
        walk(dir, &mut paths);
    }
    if pins.exists() {
        paths.push(pins.to_path_buf());
    }
    paths.into_iter().find_map(|path| {
        let mode = std::fs::metadata(&path).ok()?.mode();
        (mode & 0o022 != 0)
            .then(|| format!("{} can be written by its group or others", path.display()))
    })
}

#[cfg(not(unix))]
fn writable_by_others(_system: &Path, _dir: &Path, _pins: &Path) -> Option<String> {
    None
}

/// The directory and everything below it.
#[cfg(unix)]
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    out.push(dir.to_path_buf());
    for path in std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
    {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_system_directory_others_can_write_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(rules.join("org.toml"), "").unwrap();
        let pins = dir.path().join("packs.toml");
        for path in [dir.path(), rules.as_path()] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(
            rules.join("org.toml"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(writable_by_others(dir.path(), &rules, &pins), None);

        std::fs::set_permissions(
            rules.join("org.toml"),
            std::fs::Permissions::from_mode(0o664),
        )
        .unwrap();
        let why = writable_by_others(dir.path(), &rules, &pins).unwrap();
        assert!(
            why.ends_with("org.toml can be written by its group or others"),
            "{why}"
        );
    }
}
