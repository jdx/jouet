//! `jouet rule trust|untrust`: which repositories' and system directories'
//! rules run.

use super::super::Output;
use crate::RuleTrustArgs;
use crate::rules::{self, Kind, Load, Set, packs};
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

#[derive(Serialize)]
struct Trusted {
    dir: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    trusted: bool,
    rules: Vec<Checked>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    packs: Vec<String>,
}

/// The repository's set of rules.
fn target(cwd: &Path) -> Result<Set, (Exit, String)> {
    let mut errors = Vec::new();
    let sets = rules::sets(cwd, None, &mut rules::digests(), &mut errors);
    sets.into_iter()
        .find(|s| s.kind == Kind::Repo)
        .ok_or_else(|| {
            (
                Exit::NotFound,
                "no .jouet/rules directory or .jouet/packs.toml at the root of this repository"
                    .into(),
            )
        })
}

pub(super) fn trust(args: &RuleTrustArgs, cwd: &Path, out: &Output) -> Exit {
    if args.list {
        return list(cwd, out);
    }
    let set = match target(cwd) {
        Ok(set) => set,
        Err((exit, message)) => return out.error(exit, &message),
    };
    if let Some(exit) = needs_person(args.yes, "trusting rules", out) {
        return exit;
    }
    let dir = set.dir.display().to_string();
    // The packs it pins are part of what is trusted: fetch them first.
    let mut digests = rules::digests();
    let pinned = match set.pins.as_deref().map(packs::read).transpose() {
        Ok(pinned) => pinned.unwrap_or_default(),
        Err(e) => return out.error(Exit::Failed, &e),
    };
    for (name, pin) in &pinned.pack {
        if let Err(e) = packs::sync(pin, &mut digests) {
            return out.error(Exit::Remote, &format!("fetching rule pack {name}: {e}"));
        }
    }
    let mut loaded = rules::load_with(
        cwd,
        &Load {
            assume: Some(&set.dir),
            ungated: true,
        },
    );
    loaded.engine.load_scripts();
    let pack_dirs: Vec<PathBuf> = loaded
        .sets
        .iter()
        .filter(|s| s.kind == Kind::Pack && s.pinned_by == Some(set.kind))
        .map(|s| s.dir.clone())
        .collect();
    let mut dirs: Vec<&Path> = vec![&set.dir];
    dirs.extend(pack_dirs.iter().map(PathBuf::as_path));
    let checked = check(&loaded.engine, &dirs);
    let failed = checked.iter().any(|r| r.problem.is_some());
    // What was checked must be what is trusted.
    let digest = rules::digests().of(&set.dir, set.pins.as_deref());
    let changed = digest.as_ref().ok() != set.digest.as_ref();
    let report = Trusted {
        dir,
        trusted: !failed && !changed,
        rules: checked,
        packs: pinned.pack.keys().cloned().collect(),
    };
    if failed {
        out.result(&report, || {
            format!(
                "{}not trusting {}: every rule must load and pass its examples of both kinds first\n",
                checked_text(&report.rules),
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
    if let Err(e) = rules::trust_store().trust_digest(&set.dir, digest.clone()) {
        return out.error(
            Exit::Failed,
            &format!("updating trust for {}: {e}", report.dir),
        );
    }
    rules::remember_proven(&digest, &[]);
    for pin in pinned.pack.values() {
        rules::remember_proven(&pin.digest, &[]);
    }
    out.result(&report, || {
        let packs = if report.packs.is_empty() {
            String::new()
        } else {
            format!(" and the packs {}", report.packs.join(", "))
        };
        format!(
            "{}trusted {} ({} rules{packs}); a change to any of them needs trusting again\n",
            checked_text(&report.rules),
            report.dir,
            report.rules.len()
        )
    });
    Exit::Ok
}

#[derive(Serialize)]
struct Untrusted {
    dir: String,
    trusted: bool,
}

pub(super) fn untrust(cwd: &Path, out: &Output) -> Exit {
    let set = match target(cwd) {
        Ok(set) => set,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let was = match rules::trust_store().untrust(&set.dir) {
        Ok(was) => was,
        Err(e) => {
            return out.error(
                Exit::Failed,
                &format!("updating trust for {}: {e}", set.dir.display()),
            );
        }
    };
    let report = Untrusted {
        dir: set.dir.display().to_string(),
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
    /// This repository's rules and the system's, and whether they run.
    here: Vec<Here>,
    /// Every directory trusted, and how many versions of its rules.
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
    let sets = rules::sets(cwd, None, &mut rules::digests(), &mut errors);
    let here = sets
        .iter()
        .filter(|s| matches!(s.kind, Kind::Repo | Kind::System))
        .map(|s| Here {
            kind: if s.kind == Kind::Repo {
                "repo"
            } else {
                "system"
            },
            dir: s.dir.display().to_string(),
            state: s.status.name(),
            why: s.why.clone(),
        })
        .collect();
    let store = rules::trust_store();
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
