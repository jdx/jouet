//! `jouet rule pack add|rm|ls|sync`: rule packs, pinned by commit.

use super::super::Output;
use super::trust::{Checked, check, checked_text, needs_person};
use crate::rules::{self, Kind, Status, packs};
use crate::{RulePackAddArgs, RulePackArgs, RulePackCmd, RulePackRmArgs};
use jouet_output::exit::Exit;
use jouet_rules::{Engine, Origin, RuleDir};
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(super) fn run(args: &RulePackArgs, cwd: &Path, out: &Output) -> Exit {
    match &args.cmd {
        RulePackCmd::Add(add_args) => add(add_args, cwd, out),
        RulePackCmd::Rm(rm_args) => rm(rm_args, cwd, out),
        RulePackCmd::Ls(_) => ls(cwd, out),
        RulePackCmd::Sync(_) => sync(cwd, out),
    }
}

/// The `packs.toml` a pack is pinned in: the user's, or with `repo`, this
/// repository's.
fn pins_file(cwd: &Path, repo: bool) -> Result<PathBuf, (Exit, String)> {
    if repo {
        let root = cwd
            .ancestors()
            .find(|dir| dir.join(".git").exists())
            .ok_or((
                Exit::Precondition,
                "--repo needs a repository here".to_string(),
            ))?;
        return Ok(root.join(".jouet").join("packs.toml"));
    }
    jouet_output::paths::config_dir()
        .map(|d| d.join("packs.toml"))
        .ok_or((
            Exit::Failed,
            "there is no home directory to pin packs in; set HOME or XDG_CONFIG_HOME".into(),
        ))
}

#[derive(Serialize)]
struct Added {
    name: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    digest: String,
    pinned_in: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    rules: Vec<Checked>,
}

fn add(args: &RulePackAddArgs, cwd: &Path, out: &Output) -> Exit {
    if !jouet_rules::valid_name(&args.name) {
        return out.error(
            Exit::Usage,
            &format!(
                "`{}` is not a pack name: lowercase words joined by dashes",
                args.name
            ),
        );
    }
    let pins_path = match pins_file(cwd, args.repo) {
        Ok(path) => path,
        Err((exit, message)) => return out.error(exit, &message),
    };
    if let Some(exit) = needs_person(args.yes, "adding a rule pack", out) {
        return exit;
    }
    let git = packs::is_git_url(&args.source) || args.rev.is_some();
    let fetched = match packs::fetch(
        &args.source,
        git,
        args.rev.as_deref(),
        args.dir.as_deref(),
        None,
    ) {
        Ok(fetched) => fetched,
        Err(e) => {
            let exit = if git { Exit::Remote } else { Exit::Failed };
            return out.error(exit, &format!("fetching {}: {e}", args.source));
        }
    };
    // Its rules, on their own, as a pack's: each must pass its examples.
    let (sources, errors) = jouet_rules::discover(&[RuleDir {
        dir: fetched.dir.clone(),
        origin: Origin::Pack,
        trusted: true,
    }]);
    for error in errors {
        eprintln!("jouet: {error}");
    }
    let mut engine = Engine::new(
        sources,
        jouet_rules::Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    );
    engine.load_scripts();
    let checked = check(&engine, &[&fetched.dir]);
    let failed = checked.iter().any(|r| r.problem.is_some());
    let mut report = Added {
        name: args.name.clone(),
        source: fetched.pin.source(),
        commit: fetched.pin.rev.clone(),
        digest: fetched.pin.digest.clone(),
        pinned_in: pins_path.display().to_string(),
        pinned: false,
        rules: checked,
    };
    if failed {
        out.result(&report, || {
            format!(
                "{}not pinning {}: every rule must load and pass its examples of both kinds first\n",
                checked_text(&report.rules),
                report.name
            )
        });
        return Exit::Failed;
    }
    let mut pins = match packs::read(&pins_path) {
        Ok(pins) => pins,
        Err(e) => return out.error(Exit::Failed, &e),
    };
    pins.pack.insert(args.name.clone(), fetched.pin.clone());
    if let Err(e) = packs::write(&pins_path, &pins) {
        return out.error(Exit::Failed, &e);
    }
    rules::remember_proven(&fetched.pin.digest, &[]);
    report.pinned = true;
    out.result(&report, || {
        let mut text = checked_text(&report.rules);
        let _ = writeln!(
            text,
            "pinned {} ({}, {} rules) in {}",
            report.name,
            report.source,
            report.rules.len(),
            report.pinned_in
        );
        if args.repo {
            text.push_str(
                "this repository's rules changed, so they need trusting again: `jouet rule trust`\n",
            );
        }
        text
    });
    Exit::Ok
}

#[derive(Serialize)]
struct Removed {
    name: String,
    pinned_in: String,
    removed: bool,
}

fn rm(args: &RulePackRmArgs, cwd: &Path, out: &Output) -> Exit {
    let pins_path = match pins_file(cwd, args.repo) {
        Ok(path) => path,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let mut pins = match packs::read(&pins_path) {
        Ok(pins) => pins,
        Err(e) => return out.error(Exit::Failed, &e),
    };
    if pins.pack.remove(&args.name).is_none() {
        return out.error(
            Exit::NotFound,
            &format!("{} pins no pack named {}", pins_path.display(), args.name),
        );
    }
    if let Err(e) = packs::write(&pins_path, &pins) {
        return out.error(Exit::Failed, &e);
    }
    let report = Removed {
        name: args.name.clone(),
        pinned_in: pins_path.display().to_string(),
        removed: true,
    };
    out.result(&report, || {
        format!("unpinned {} from {}\n", report.name, report.pinned_in)
    });
    Exit::Ok
}

#[derive(Serialize)]
struct Listed {
    name: String,
    /// `user`, `repo` or `system`: whose pins name it.
    pinned_by: &'static str,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    digest: String,
    /// verified, mismatch, missing or invalid.
    state: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    enforced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
}

fn kind_name(kind: Option<Kind>) -> &'static str {
    match kind {
        Some(Kind::System) => "system",
        Some(Kind::Repo) => "repo",
        _ => "user",
    }
}

fn listed(cwd: &Path) -> (Vec<Listed>, Vec<String>) {
    let mut errors = Vec::new();
    let sets = rules::sets(cwd, None, &mut rules::digests(), &mut errors);
    let packs = sets
        .iter()
        .filter(|s| s.kind == Kind::Pack)
        .filter_map(|s| {
            let pin = s.pin.as_ref()?;
            Some(Listed {
                name: s.name.clone().unwrap_or_default(),
                pinned_by: kind_name(s.pinned_by),
                source: pin.source(),
                commit: pin.rev.clone(),
                digest: pin.digest.clone(),
                state: s.status.name(),
                enforced: s.enforced(),
                problem: match &s.status {
                    Status::Invalid(problem) => Some(problem.clone()),
                    _ => None,
                },
            })
        })
        .collect();
    // Pins in a repository not trusted are not followed; say so.
    let notes = sets
        .iter()
        .filter(|s| {
            !s.status.runs() && s.pins.as_deref().is_some_and(Path::is_file)
        })
        .map(|s| {
            format!(
                "{} pins packs, which are not fetched or run until it is trusted (`jouet rule trust`)",
                s.dir.display()
            )
        })
        .chain(errors)
        .collect();
    (packs, notes)
}

#[derive(Serialize)]
struct Ls {
    packs: Vec<Listed>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

fn ls(cwd: &Path, out: &Output) -> Exit {
    let (packs, notes) = listed(cwd);
    let report = Ls { packs, notes };
    out.result(&report, || {
        let mut text = String::new();
        if report.packs.is_empty() {
            text.push_str("no rule packs are pinned\n");
        }
        for p in &report.packs {
            let enforced = if p.enforced { "  enforced" } else { "" };
            let _ = writeln!(
                text,
                "{}  {}  {}  {}{enforced}",
                p.name, p.state, p.pinned_by, p.source
            );
            if let Some(problem) = &p.problem {
                let _ = writeln!(text, "  {problem}");
            }
        }
        for note in &report.notes {
            let _ = writeln!(text, "note: {note}");
        }
        text
    });
    Exit::Ok
}

#[derive(Serialize)]
struct Synced {
    name: String,
    fetched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Sync {
    packs: Vec<Synced>,
}

fn sync(cwd: &Path, out: &Output) -> Exit {
    let mut errors = Vec::new();
    let mut digests = rules::digests();
    let sets = rules::sets(cwd, None, &mut digests, &mut errors);
    for error in &errors {
        eprintln!("jouet: {error}");
    }
    let packs: Vec<Synced> = sets
        .iter()
        .filter_map(|s| Some((s.name.clone()?, s.pin.as_ref()?)))
        .map(|(name, pin)| match packs::sync(pin, &mut digests) {
            Ok(fetched) => Synced {
                name,
                fetched,
                error: None,
            },
            Err(e) => Synced {
                name,
                fetched: false,
                error: Some(e),
            },
        })
        .collect();
    digests.save();
    let failed = packs.iter().any(|p| p.error.is_some());
    let report = Sync { packs };
    out.result(&report, || {
        let mut text = String::new();
        if report.packs.is_empty() {
            text.push_str("no rule packs are pinned\n");
        }
        for p in &report.packs {
            let _ = match (&p.error, p.fetched) {
                (Some(e), _) => writeln!(text, "FAIL  {}  {e}", p.name),
                (None, true) => writeln!(text, "ok    {}  fetched", p.name),
                (None, false) => writeln!(text, "ok    {}  kept as pinned", p.name),
            };
        }
        text
    });
    if failed { Exit::Remote } else { Exit::Ok }
}
