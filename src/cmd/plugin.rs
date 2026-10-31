//! `jouet plugin add|rm|ls|sync`: plugins of rules and routines, pinned by
//! commit.

use super::Output;
use super::trust::{Checked, check, checked_text, cwd, needs_person};
use crate::plugins;
use crate::sources::{self, Kind, Status};
use crate::{PluginAddArgs, PluginArgs, PluginCmd, PluginRmArgs};
use jouet_output::exit::Exit;
use jouet_rules::{Engine, Origin, RuleDir};
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) fn run(args: &PluginArgs, out: &Output) -> Exit {
    let cwd = match cwd(out) {
        Ok(cwd) => cwd,
        Err(exit) => return exit,
    };
    match &args.cmd {
        PluginCmd::Add(add_args) => add(add_args, &cwd, out),
        PluginCmd::Rm(rm_args) => rm(rm_args, &cwd, out),
        PluginCmd::Ls(_) => ls(&cwd, out),
        PluginCmd::Sync(_) => sync(&cwd, out),
    }
}

/// Where else a plugin of this name is pinned or installed, other than the
/// `plugins.toml` it is being added to: the system's, the user's, or this
/// repository's, trusted or not.
fn name_taken(cwd: &Path, name: &str, repo: bool) -> Option<String> {
    let mut errors = Vec::new();
    let sets = sources::sets(cwd, None, &mut sources::digests(), &mut errors);
    let into = if repo { Kind::Repo } else { Kind::User };
    if let Some(other) = sets.iter().find(|s| {
        s.kind == Kind::Plugin && s.name.as_deref() == Some(name) && s.pinned_by != Some(into)
    }) {
        return Some(sources::whose(other));
    }
    // An untrusted repository's pins are not among the sets.
    let repo_pins = sources::repo_root(cwd).map(|root| root.join("plugins.toml"))?;
    (!repo && plugins::read(&repo_pins).is_ok_and(|p| p.plugin.contains_key(name)))
        .then(|| format!("pinned by {}", repo_pins.display()))
}

/// The `plugins.toml` a plugin is pinned in: the user's, or with `repo`,
/// this repository's.
fn pins_file(cwd: &Path, repo: bool) -> Result<PathBuf, (Exit, String)> {
    if repo {
        return sources::repo_root(cwd)
            .map(|root| root.join("plugins.toml"))
            .ok_or((
                Exit::Precondition,
                "--repo needs a repository here".to_string(),
            ));
    }
    jouet_output::paths::config_dir()
        .map(|d| d.join("plugins.toml"))
        .ok_or((
            Exit::Failed,
            "there is no home directory to pin plugins in; set HOME or XDG_CONFIG_HOME".into(),
        ))
}

#[derive(Serialize)]
struct Added {
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    digest: String,
    pinned_in: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    rules: Vec<Checked>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    routines: Vec<String>,
}

fn add(args: &PluginAddArgs, cwd: &Path, out: &Output) -> Exit {
    let pins_path = match pins_file(cwd, args.repo) {
        Ok(path) => path,
        Err((exit, message)) => return out.error(exit, &message),
    };
    if let Some(exit) = needs_person(args.yes, "adding a plugin", out) {
        return exit;
    }
    let git = plugins::is_git_url(&args.source) || args.rev.is_some();
    let fetched = match plugins::fetch(
        &args.source,
        git,
        args.rev.as_deref(),
        args.dir.as_deref(),
        None,
    ) {
        Ok(fetched) => fetched,
        Err(e) => {
            let exit = if git { Exit::Remote } else { Exit::Failed };
            return out.error(exit, &format!("adding {}: {e}", args.source));
        }
    };
    let name = fetched.manifest.name.clone();
    if let Some(other) = name_taken(cwd, &name, args.repo) {
        return out.error_with_hint(
            Exit::Precondition,
            &format!("there is already a plugin named {name}, {other}"),
            Some("a plugin's rules and routines are named after it, so its name must be its own; unpin the other first, or add this one to the same plugins.toml to replace it"),
        );
    }
    // Its rules, on their own, as a plugin's: each must pass its examples.
    let (rule_sources, errors) = jouet_rules::discover(&[RuleDir {
        dir: fetched.root.join("rules"),
        origin: Origin::Plugin,
        trusted: true,
        namespace: Some(name.clone()),
    }]);
    for error in errors {
        eprintln!("jouet: {error}");
    }
    let mut engine = Engine::new(
        rule_sources,
        jouet_rules::Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    );
    engine.load_scripts();
    let checked = check(&engine, &[&fetched.root.join("rules")]);
    let failed = checked.iter().any(|r| r.problem.is_some());
    let routines = jouet_routines::routine::luau_files(&fetched.root.join("routines"))
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.file_stem())
        .map(|stem| format!("{name}/{}", stem.to_string_lossy()))
        .collect();
    let mut report = Added {
        name: name.clone(),
        description: fetched.manifest.description.clone(),
        source: fetched.pin.source(),
        commit: fetched.pin.rev.clone(),
        digest: fetched.pin.digest.clone(),
        pinned_in: pins_path.display().to_string(),
        pinned: false,
        rules: checked,
        routines,
    };
    let reviewed = |report: &Added| {
        let mut text = checked_text(&report.rules);
        if !report.routines.is_empty() {
            let _ = writeln!(text, "routines: {}", report.routines.join(", "));
        }
        text
    };
    if failed {
        out.result(&report, || {
            format!(
                "{}not pinning {}: every rule must load and pass its examples of both kinds first\n",
                reviewed(&report),
                report.name
            )
        });
        return Exit::Failed;
    }
    let mut pins = match plugins::read(&pins_path) {
        Ok(pins) => pins,
        Err(e) => return out.error(Exit::Failed, &e),
    };
    pins.plugin.insert(name, fetched.pin.clone());
    if let Err(e) = plugins::write(&pins_path, &pins) {
        return out.error(Exit::Failed, &e);
    }
    crate::rules::remember_proven(&fetched.pin.digest);
    report.pinned = true;
    out.result(&report, || {
        let mut text = reviewed(&report);
        let _ = writeln!(
            text,
            "pinned {} ({}, {} rules, {} routines) in {}",
            report.name,
            report.source,
            report.rules.len(),
            report.routines.len(),
            report.pinned_in
        );
        if args.repo {
            text.push_str(
                "this repository's plugins changed, so it needs trusting again: `jouet trust`\n",
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

fn rm(args: &PluginRmArgs, cwd: &Path, out: &Output) -> Exit {
    let pins_path = match pins_file(cwd, args.repo) {
        Ok(path) => path,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let mut pins = match plugins::read(&pins_path) {
        Ok(pins) => pins,
        Err(e) => return out.error(Exit::Failed, &e),
    };
    if pins.plugin.remove(&args.name).is_none() {
        return out.error(
            Exit::NotFound,
            &format!("{} pins no plugin named {}", pins_path.display(), args.name),
        );
    }
    if let Err(e) = plugins::write(&pins_path, &pins) {
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
    /// `user`, `repo` or `system`: whose pins name it, or the system's
    /// `plugins/` for one installed there.
    pinned_by: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    /// verified, enforced, mismatch, missing, needs, invalid or writable.
    state: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    enforced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
}

fn listed(cwd: &Path) -> (Vec<Listed>, Vec<String>) {
    let mut errors = Vec::new();
    let sets = sources::sets(cwd, None, &mut sources::digests(), &mut errors);
    let plugins = sets
        .iter()
        .filter(|s| s.kind == Kind::Plugin)
        .map(|s| Listed {
            name: s.name.clone().unwrap_or_default(),
            pinned_by: s.pinned_by.map_or("user", Kind::name),
            source: s
                .pin
                .as_ref()
                .map_or_else(|| s.root.display().to_string(), plugins::Pin::source),
            commit: s.pin.as_ref().and_then(|p| p.rev.clone()),
            digest: s.digest.clone(),
            state: s.status.name(),
            enforced: s.enforced(),
            problem: match &s.status {
                Status::Invalid(problem) => Some(problem.clone()),
                Status::Needs(version) => Some(format!("needs jouet {version} or newer")),
                Status::Writable => s.why.clone(),
                _ => None,
            },
        })
        .collect();
    // Pins in a repository not trusted are not followed; say so.
    let notes = sets
        .iter()
        .filter(|s| !s.status.runs() && s.pins().as_deref().is_some_and(Path::is_file))
        .map(|s| {
            format!(
                "{} pins plugins, which are not fetched or run until it is trusted (`jouet trust`)",
                s.root.display()
            )
        })
        .chain(errors)
        .collect();
    (plugins, notes)
}

#[derive(Serialize)]
struct Ls {
    plugins: Vec<Listed>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

fn ls(cwd: &Path, out: &Output) -> Exit {
    let (plugins, notes) = listed(cwd);
    let report = Ls { plugins, notes };
    out.result(&report, || {
        let mut text = String::new();
        if report.plugins.is_empty() {
            text.push_str("no plugins are pinned\n");
        }
        for p in &report.plugins {
            let enforced = if p.enforced && p.state != "enforced" {
                "  enforced"
            } else {
                ""
            };
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    fetched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Sync {
    plugins: Vec<Synced>,
}

fn sync(cwd: &Path, out: &Output) -> Exit {
    let mut errors = Vec::new();
    let mut digests = sources::digests();
    let sets = sources::sets(cwd, None, &mut digests, &mut errors);
    for error in &errors {
        eprintln!("jouet: {error}");
    }
    let plugins: Vec<Synced> = sets
        .iter()
        .filter_map(|s| Some((s.name.clone()?, s.pin.as_ref()?)))
        .map(|(name, pin)| match plugins::sync(pin, &mut digests) {
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
    let failed = plugins.iter().any(|p| p.error.is_some());
    let report = Sync { plugins };
    out.result(&report, || {
        let mut text = String::new();
        if report.plugins.is_empty() {
            text.push_str("no plugins are pinned\n");
        }
        for p in &report.plugins {
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
