//! `jouet trust` and `jouet untrust`: whether this repository's rules,
//! routines and plugins run.

use super::Output;
use crate::cli::TrustArgs;
use crate::plugins;
use crate::rules::{self, Load};
use crate::sources::{self, Kind, Set};
use jouet_output::exit::Exit;
use jouet_rules::{Engine, State};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// Refuses a decision only the user makes when no one is at a terminal.
pub(super) fn needs_person(yes: bool, what: &str, out: &Output) -> Option<Exit> {
    if yes || std::io::stdin().is_terminal() {
        return None;
    }
    Some(out.error_with_hint(
        Exit::Precondition,
        &format!("{what} is the user's decision, and there is no terminal to make it at"),
        Some("the user runs it in a terminal, or passes --yes from a script or CI job they run"),
    ))
}

/// One rule from outside, checked before it may run.
#[derive(Serialize)]
pub(super) struct Checked {
    pub name: String,
    pub mode: jouet_rules::Mode,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub why: String,
    pub source: String,
    /// Why it may not run: it does not load, or does not pass its examples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// Every rule from a file in one of `dirs`, and whether it loads and passes
/// its examples of both kinds.
pub(super) fn check(engine: &Engine, dirs: &[&Path]) -> Vec<Checked> {
    engine
        .rules()
        .iter()
        .filter(|r| {
            r.origin
                .path()
                .is_some_and(|p| dirs.iter().any(|d| p.starts_with(d)))
        })
        .map(|r| Checked {
            name: r.name.clone(),
            mode: r.mode,
            why: r.why.clone(),
            source: r.origin.describe(),
            problem: match &r.state {
                State::Broken(e) => Some(format!("it does not load: {e}")),
                _ => engine.proven(r).err().map(|e| {
                    format!(
                        "its examples do not pass: {e}; see `jouet rule test {}`",
                        r.name
                    )
                }),
            },
        })
        .collect()
}

/// The rules as a person reviews them, and what is wrong with any.
pub(super) fn checked_text(rules: &[Checked]) -> String {
    let width = rules
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    for r in rules {
        let mode = match r.mode {
            jouet_rules::Mode::Warn => "warn",
            jouet_rules::Mode::Deny => "deny",
        };
        let mark = if r.problem.is_some() { "FAIL" } else { "ok  " };
        let _ = writeln!(text, "{mark}  {:width$}  {mode}  {}", r.name, r.why);
        if let Some(problem) = &r.problem {
            let _ = writeln!(text, "      {problem}");
        }
    }
    text
}

/// The current directory, or the exit for when it cannot be read.
pub(super) fn cwd(out: &Output) -> Result<PathBuf, Exit> {
    std::env::current_dir()
        .map_err(|e| out.error(Exit::Failed, &format!("reading the current directory: {e}")))
}

/// The names of the routines in a directory, as `joue` runs them.
fn routines(dir: &Path, namespace: Option<&str>) -> Vec<String> {
    jouet_routines::routine::luau_files(dir)
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.file_stem())
        .map(|stem| match namespace {
            Some(ns) => format!("{ns}/{}", stem.to_string_lossy()),
            None => stem.to_string_lossy().into_owned(),
        })
        .collect()
}

#[derive(Serialize)]
struct Trusted {
    dir: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    trusted: bool,
    rules: Vec<Checked>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    routines: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    plugins: Vec<String>,
}

/// This repository's set.
fn target(cwd: &Path) -> Result<Set, (Exit, String)> {
    let mut errors = Vec::new();
    let sets = sources::sets(cwd, None, &mut sources::digests(), &mut errors);
    sets.into_iter()
        .find(|s| s.kind == Kind::Repo)
        .ok_or_else(|| {
            (
            Exit::NotFound,
            "no .jouet/rules, .jouet/routines or .jouet/plugins.toml at the root of this repository"
                .into(),
        )
        })
}

pub(crate) fn trust(args: &TrustArgs, out: &Output) -> Exit {
    let cwd = match cwd(out) {
        Ok(cwd) => cwd,
        Err(exit) => return exit,
    };
    if args.list {
        return list(&cwd, out);
    }
    let set = match target(&cwd) {
        Ok(set) => set,
        Err((exit, message)) => return out.error(exit, &message),
    };
    if let Some(exit) = needs_person(args.yes, "trusting a repository's rules and routines", out) {
        return exit;
    }
    let dir = set.root.display().to_string();
    // The plugins it pins are part of what is trusted: fetch them first.
    let mut digests = sources::digests();
    let pinned = match set.pins().map(|p| plugins::read(&p)).transpose() {
        Ok(pinned) => pinned.unwrap_or_default(),
        Err(e) => return out.error(Exit::Failed, &e),
    };
    for (name, pin) in &pinned.plugin {
        if let Err(e) = plugins::sync(pin, &mut digests) {
            return out.error(Exit::Remote, &format!("fetching plugin {name}: {e}"));
        }
    }
    let mut loaded = rules::load_with(
        &cwd,
        &Load {
            assume: Some(&set.root),
            ungated: true,
        },
    );
    loaded.engine.load_scripts();
    let plugins: Vec<&Set> = loaded
        .sets
        .iter()
        .filter(|s| s.kind == Kind::Plugin && s.pinned_by == Some(Kind::Repo))
        .collect();
    // A plugin that could not be kept as pinned is not trusted blind.
    if let Some(bad) = plugins.iter().find(|s| !s.status.runs()) {
        return out.error(
            Exit::Failed,
            &format!(
                "{} is {}, so it cannot be checked",
                bad.label(),
                bad.status.name()
            ),
        );
    }
    let mut rule_dirs = vec![set.rules_dir()];
    rule_dirs.extend(plugins.iter().map(|s| s.rules_dir()));
    let dirs: Vec<&Path> = rule_dirs.iter().map(PathBuf::as_path).collect();
    let checked = check(&loaded.engine, &dirs);
    let mut routine_names = routines(&set.routines_dir(), None);
    for plugin in &plugins {
        routine_names.extend(routines(&plugin.routines_dir(), plugin.name.as_deref()));
    }
    let failed = checked.iter().any(|r| r.problem.is_some());
    // What was checked must be what is trusted.
    let digest = sources::digests().of(&set.root, sources::ENTRIES);
    let changed = digest.as_ref().ok() != set.digest.as_ref();
    let report = Trusted {
        dir,
        trusted: !failed && !changed,
        rules: checked,
        routines: routine_names,
        plugins: pinned.plugin.keys().cloned().collect(),
    };
    let reviewed = |report: &Trusted| {
        let mut text = checked_text(&report.rules);
        if !report.routines.is_empty() {
            let _ = writeln!(text, "routines: {}", report.routines.join(", "));
        }
        text
    };
    if failed {
        out.result(&report, || {
            format!(
                "{}not trusting {}: every rule must load and pass its examples of both kinds first\n",
                reviewed(&report),
                report.dir
            )
        });
        return Exit::Failed;
    }
    let digest = match digest {
        Ok(digest) if !changed => digest,
        _ => {
            return out.error(
                Exit::Failed,
                &format!("{} changed while it was checked; run it again", report.dir),
            );
        }
    };
    if let Err(e) = sources::trust_store().trust(&set.root, digest.clone()) {
        return out.error(
            Exit::Failed,
            &format!("updating trust for {}: {e}", report.dir),
        );
    }
    rules::remember_proven(&digest);
    for pin in pinned.plugin.values() {
        rules::remember_proven(&pin.digest);
    }
    out.result(&report, || {
        let plugins = if report.plugins.is_empty() {
            String::new()
        } else {
            format!(" and the plugins {}", report.plugins.join(", "))
        };
        format!(
            "{}trusted {} ({} rules, {} routines{plugins}); a change to any of them needs trusting again\n",
            reviewed(&report),
            report.dir,
            report.rules.len(),
            report.routines.len()
        )
    });
    Exit::Ok
}

#[derive(Serialize)]
struct Untrusted {
    dir: String,
    trusted: bool,
}

pub(crate) fn untrust(out: &Output) -> Exit {
    let cwd = match cwd(out) {
        Ok(cwd) => cwd,
        Err(exit) => return exit,
    };
    let set = match target(&cwd) {
        Ok(set) => set,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let was = match sources::trust_store().untrust(&set.root) {
        Ok(was) => was,
        Err(e) => {
            return out.error(
                Exit::Failed,
                &format!("updating trust for {}: {e}", set.root.display()),
            );
        }
    };
    let report = Untrusted {
        dir: set.root.display().to_string(),
        trusted: false,
    };
    out.result(&report, || {
        if was {
            format!("no longer trusting {}\n", report.dir)
        } else {
            format!("{} was not trusted\n", report.dir)
        }
    });
    Exit::Ok
}

#[derive(Serialize)]
struct Listed {
    /// This repository's set and the system's, and whether they run.
    here: Vec<Here>,
    /// Every directory trusted, and how many versions of its files.
    trusted: Vec<Entry>,
}

#[derive(Serialize)]
struct Here {
    kind: &'static str,
    dir: String,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
}

#[derive(Serialize)]
struct Entry {
    dir: String,
    versions: usize,
}

fn list(cwd: &Path, out: &Output) -> Exit {
    let mut errors = Vec::new();
    let sets = sources::sets(cwd, None, &mut sources::digests(), &mut errors);
    let here = sets
        .iter()
        .filter(|s| matches!(s.kind, Kind::Repo | Kind::System))
        .map(|s| Here {
            kind: s.kind.name(),
            dir: s.root.display().to_string(),
            state: s.status.name(),
            why: s.why.clone(),
        })
        .collect();
    let store = sources::trust_store();
    let report = Listed {
        here,
        trusted: store
            .entries()
            .map(|(dir, versions)| Entry {
                dir: dir.to_string(),
                versions,
            })
            .collect(),
    };
    out.result(&report, || {
        let mut text = String::new();
        for h in &report.here {
            let _ = writeln!(text, "{:<6}  {:<9}  {}", h.kind, h.state, h.dir);
        }
        if report.trusted.is_empty() {
            text.push_str("nothing is trusted\n");
        }
        for e in &report.trusted {
            let versions = if e.versions == 1 {
                "1 version".to_string()
            } else {
                format!("{} versions", e.versions)
            };
            let _ = writeln!(text, "trusted  {}  ({versions})", e.dir);
        }
        text
    });
    for error in errors {
        eprintln!("jouet: {error}");
    }
    Exit::Ok
}
