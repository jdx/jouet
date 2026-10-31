//! Where rules and routines come from, besides jouet itself: the system
//! directory, the user's, a repository's `.jouet/`, and the plugins each of
//! those pins, with whether each set runs.

use crate::plugins::{self, Manifest, Pin};
use jouet_rules::{Digests, Trust, TrustStore};
use std::path::{Path, PathBuf};

/// The files a repository's or the system's trust and hash cover.
pub(crate) const ENTRIES: &[&str] = &["rules", "routines", "plugins.toml"];

/// Where a set comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The system directory, `/etc/jouet`: the organisation's.
    System,
    User,
    Repo,
    Plugin,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Repo => "repo",
            Self::Plugin => "plugin",
        }
    }
}

/// Whether a set runs, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    /// The user's own.
    Own,
    /// The system directory's: the organisation's.
    Enforced,
    /// The system directory, when its group or others can write it.
    Writable,
    Trusted,
    /// Trusted once, but changed since.
    Changed,
    Untrusted,
    /// A plugin whose kept files hash to its pin.
    Verified,
    /// A plugin whose kept files do not hash to its pin.
    Mismatch,
    /// A plugin not fetched yet.
    Missing,
    /// A plugin that needs a newer jouet.
    Needs(String),
    /// A pin or a plugin that cannot be followed.
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
            Self::Needs(_) => "needs",
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

/// A directory with `rules/` and `routines/`: the system's, the user's, a
/// repository's `.jouet/`, or a plugin's.
#[derive(Debug, Clone)]
pub(crate) struct Set {
    pub kind: Kind,
    /// A plugin's name.
    pub name: Option<String>,
    /// For a plugin, the kind of set that pins it.
    pub pinned_by: Option<Kind>,
    pub root: PathBuf,
    pub status: Status,
    /// Why the system directory's rules do not run.
    pub why: Option<String>,
    /// The hash of its files, as trust records it.
    pub digest: Option<String>,
    /// A plugin's pin; none for one in the system's `plugins/`.
    pub pin: Option<Pin>,
}

impl Set {
    pub(crate) fn rules_dir(&self) -> PathBuf {
        self.root.join("rules")
    }

    pub(crate) fn routines_dir(&self) -> PathBuf {
        self.root.join("routines")
    }

    /// The `plugins.toml` of a set that can pin plugins.
    pub(crate) fn pins(&self) -> Option<PathBuf> {
        (self.kind != Kind::Plugin).then(|| self.root.join("plugins.toml"))
    }

    /// Whether its rules and routines run as enforced: the system's, or a
    /// plugin it pins.
    pub(crate) fn enforced(&self) -> bool {
        self.kind == Kind::System || self.pinned_by == Some(Kind::System)
    }

    /// The namespace its rules' and routines' names start with: a plugin's
    /// name.
    pub(crate) fn namespace(&self) -> Option<String> {
        (self.kind == Kind::Plugin)
            .then(|| self.name.clone())
            .flatten()
    }

    /// Whether `path`, a rule's or routine's file, is one of this set's.
    pub(crate) fn holds(&self, path: &Path) -> bool {
        path.starts_with(self.rules_dir()) || path.starts_with(self.routines_dir())
    }

    /// Whether there is anything to load from it.
    pub(crate) fn loads(&self) -> bool {
        !matches!(
            self.status,
            Status::Missing | Status::Invalid(_) | Status::Needs(_)
        )
    }

    /// How the set is named to a person: `.jouet`, `plugin acme`.
    pub(crate) fn label(&self) -> String {
        match (self.kind, &self.name) {
            (Kind::Plugin, Some(name)) => format!("plugin {name}"),
            _ => self.root.display().to_string(),
        }
    }

    /// The scope column's text for its rules and routines, such as
    /// `repo:trusted`, `plugin:acme`, `plugin:acme:mismatch`, `enforced`, or
    /// `enforced:acme` for a plugin the system pins.
    pub(crate) fn scope(&self) -> String {
        let mut text = if self.enforced() {
            "enforced".to_string()
        } else {
            self.kind.name().to_string()
        };
        if let Some(name) = &self.name {
            text.push(':');
            text.push_str(name);
        }
        if !matches!(
            self.status,
            Status::Own | Status::Enforced | Status::Verified
        ) {
            text.push(':');
            text.push_str(self.status.name());
        }
        text
    }
}

/// What to do about each set that does not run, for `jouet rule list` and
/// `joue --list`.
pub(crate) fn notes(sets: &[Set]) -> Vec<String> {
    sets.iter()
        .filter_map(|s| {
            let root = s.root.display();
            let label = s.label();
            Some(match &s.status {
                Status::Untrusted => format!(
                    "{root} is not trusted, so its rules and routines do not run; review them, then run `jouet trust`"
                ),
                Status::Changed => format!(
                    "{root} changed since it was trusted, so its rules and routines do not run; review them, then run `jouet trust`"
                ),
                Status::Writable => format!(
                    "{}, so the enforced rules and routines of {label} do not run until only its owner can write them",
                    s.why.as_deref().unwrap_or_default()
                ),
                Status::Missing => format!(
                    "{label} ({}) is not fetched, so it does not run; run `jouet plugin sync`",
                    s.pin.as_ref().map(Pin::source).unwrap_or_default()
                ),
                Status::Mismatch => format!(
                    "{label}: the files kept in {root} do not match its pin, so it does not run; run `jouet plugin sync`"
                ),
                Status::Needs(version) => {
                    format!("{label} needs jouet {version} or newer, so it does not run")
                }
                Status::Invalid(problem) => format!("{label}: {problem}"),
                Status::Own | Status::Enforced | Status::Trusted | Status::Verified => {
                    return None;
                }
            })
        })
        .collect()
}

/// `<repository root>/.jouet`, when there is a repository above `cwd`.
pub(crate) fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(|root| root.join(".jouet"))
}

/// `$JOUET_SYSTEM_DIR`, else `/etc/jouet` (`%ProgramData%\jouet` on
/// Windows): the organisation's rules, routines and plugins, which the
/// user's settings cannot turn off.
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

pub(crate) fn trust_store() -> TrustStore {
    TrustStore::load(jouet_output::paths::state_dir().map(|d| d.join("trusted.json")))
}

/// The hashes of sets, cached by the files' stat.
pub(crate) fn digests() -> Digests {
    Digests::load(jouet_output::paths::cache_dir().map(|d| d.join("digests.json")))
}

/// Whether there is a set at `root`.
fn present(root: &Path) -> bool {
    ENTRIES.iter().any(|e| root.join(e).exists())
}

/// Every set for `cwd`, in the order their rules and routines load: the
/// system's, then the user's, then the repository's, each followed by the
/// plugins it pins (the system's also by those in its `plugins/`). A set
/// that does not run pins nothing. `assume` is a repository root to treat
/// as trusted, as `jouet trust` checks it.
pub(crate) fn sets(
    cwd: &Path,
    assume: Option<&Path>,
    digests: &mut Digests,
    errors: &mut Vec<String>,
) -> Vec<Set> {
    let set = |kind, root: PathBuf, status, digest| Set {
        kind,
        name: None,
        pinned_by: None,
        root,
        status,
        why: None,
        digest,
        pin: None,
    };
    let mut out = Vec::new();
    if let Some(system) = system_dir().filter(|s| present(s) || s.join("plugins").is_dir()) {
        // An unreadable file shows when its rules load.
        let digest = digests.of(&system, ENTRIES).ok();
        let why = writable_by_others(&system);
        let status = if why.is_some() {
            Status::Writable
        } else {
            Status::Enforced
        };
        let runs = why.is_none();
        out.push(Set {
            why,
            ..set(Kind::System, system.clone(), status, digest)
        });
        if runs {
            out.extend(pinned(
                &system.join("plugins.toml"),
                Kind::System,
                digests,
                errors,
            ));
            out.extend(installed(&system.join("plugins"), digests, errors));
        }
    }
    if let Some(config) = jouet_output::paths::config_dir() {
        out.push(set(Kind::User, config.clone(), Status::Own, None));
        out.extend(pinned(
            &config.join("plugins.toml"),
            Kind::User,
            digests,
            errors,
        ));
    }
    if let Some(root) = repo_root(cwd).filter(|r| present(r)) {
        let (status, digest) = match digests.of(&root, ENTRIES) {
            Ok(digest) if assume.is_some_and(|a| a == root) => (Status::Trusted, Some(digest)),
            Ok(digest) => (
                Status::of(trust_store().check(&root, &digest)),
                Some(digest),
            ),
            Err(e) => {
                errors.push(format!("reading {}: {e}", root.display()));
                (Status::Untrusted, None)
            }
        };
        let runs = status.runs();
        out.push(set(Kind::Repo, root.clone(), status, digest));
        if runs {
            out.extend(pinned(
                &root.join("plugins.toml"),
                Kind::Repo,
                digests,
                errors,
            ));
        }
    }
    out
}

/// The plugins a `plugins.toml` pins, and whether each is kept as pinned.
fn pinned(pins: &Path, by: Kind, digests: &mut Digests, errors: &mut Vec<String>) -> Vec<Set> {
    let pinned = match plugins::read(pins) {
        Ok(pinned) => pinned,
        Err(e) => {
            errors.push(e);
            return Vec::new();
        }
    };
    pinned
        .plugin
        .into_iter()
        .map(|(name, pin)| {
            let root = plugins::stored(&pin).unwrap_or_default();
            let status = match pin.problem() {
                Some(problem) => Status::Invalid(problem),
                None if !root.is_dir() => Status::Missing,
                None => match digests.of(&root, plugins::ENTRIES) {
                    Ok(digest) if digest == pin.digest => manifest_status(&root, &name),
                    _ => Status::Mismatch,
                },
            };
            Set {
                kind: Kind::Plugin,
                name: Some(name),
                pinned_by: Some(by),
                root,
                status,
                why: None,
                digest: Some(pin.digest.clone()),
                pin: Some(pin),
            }
        })
        .collect()
}

/// A kept plugin's status from its manifest: verified, unless it is named
/// otherwise than its pin or needs a newer jouet.
fn manifest_status(root: &Path, name: &str) -> Status {
    match Manifest::read(root) {
        Err(e) => Status::Invalid(e),
        Ok(m) if m.name != name => Status::Invalid(format!(
            "it is pinned as {name}, but its manifest names it {}",
            m.name
        )),
        Ok(m) => m
            .needs()
            .map_or(Status::Verified, |v| Status::Needs(v.to_string())),
    }
}

/// The plugins installed in the system's `plugins/`, each a directory:
/// enforced as they are, with no pin.
fn installed(dir: &Path, digests: &mut Digests, errors: &mut Vec<String>) -> Vec<Set> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    roots.sort();
    roots
        .into_iter()
        .map(|root| {
            let dir_name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let digest = digests
                .of(&root, plugins::ENTRIES)
                .map_err(|e| errors.push(format!("reading {}: {e}", root.display())))
                .ok();
            let why = writable_by_others(&root);
            let status = match (why.is_some(), manifest_status(&root, &dir_name)) {
                (true, _) => Status::Writable,
                (false, Status::Verified) => Status::Enforced,
                (false, status) => status,
            };
            Set {
                kind: Kind::Plugin,
                name: Some(dir_name),
                pinned_by: Some(Kind::System),
                root,
                status,
                why,
                digest,
                pin: None,
            }
        })
        .collect()
}

/// Why a system set does not run: its directory, or something in it, can
/// be written by its group or others, so it is not the organisation's
/// alone.
#[cfg(unix)]
fn writable_by_others(root: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let mut paths = vec![root.to_path_buf()];
    for entry in ENTRIES {
        walk(&root.join(entry), &mut paths);
    }
    paths.into_iter().find_map(|path| {
        let mode = std::fs::metadata(&path).ok()?.mode();
        (mode & 0o022 != 0)
            .then(|| format!("{} can be written by its group or others", path.display()))
    })
}

#[cfg(not(unix))]
fn writable_by_others(_root: &Path) -> Option<String> {
    None
}

/// The path, and everything below it when it is a directory.
#[cfg(unix)]
fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    if !path.exists() {
        return;
    }
    out.push(path.to_path_buf());
    if path.is_dir() {
        for child in std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            walk(&child.path(), out);
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
        let mode = |path: &Path, mode| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        mode(dir.path(), 0o755);
        mode(&rules, 0o755);
        mode(&rules.join("org.toml"), 0o644);
        assert_eq!(writable_by_others(dir.path()), None);

        mode(&rules.join("org.toml"), 0o664);
        let why = writable_by_others(dir.path()).unwrap();
        assert!(
            why.ends_with("org.toml can be written by its group or others"),
            "{why}"
        );
    }
}
