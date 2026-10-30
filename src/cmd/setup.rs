//! `jouet setup claude|codex`: installs jouet's hooks and skill.

use super::Output;
use super::skill::{self, Format};
use crate::SetupArgs;
use crate::git;
use crate::setup::{self, Harness, Scope, has_jouet};
use jouet_output::exit::Exit;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Create,
    Update,
    Remove,
    Unchanged,
    /// A skill file setup did not write, or one edited since: left alone.
    Keep,
}

#[derive(Debug, Serialize)]
struct FileChange {
    path: String,
    action: Action,
    #[serde(skip_serializing_if = "String::is_empty")]
    diff: String,
    /// The new contents; written unless this is a dry run.
    #[serde(skip)]
    contents: Option<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    harness: &'static str,
    scope: &'static str,
    /// Installed into the repository's tracked files, for everyone.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    shared: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    uninstall: bool,
    files: Vec<FileChange>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

pub(crate) fn run(args: &SetupArgs, out: &Output) -> Exit {
    let Some(harness) = Harness::parse(&args.harness) else {
        return out.error(
            Exit::Usage,
            &format!(
                "the harness must be claude or codex, not `{}`",
                args.harness
            ),
        );
    };
    if args.global && (args.project || args.shared) {
        return out.error(
            Exit::Usage,
            "--global cannot be used with --project or --shared",
        );
    }
    let scope = if args.shared {
        Scope::Shared
    } else if args.project {
        Scope::Local
    } else {
        Scope::Global
    };
    if harness == Harness::Codex && scope == Scope::Local && !args.uninstall {
        return out.error(
            Exit::Usage,
            "Codex has no project hooks file of your own: .codex/hooks.json is the repository's, \
             shared with everyone who clones it. Use --global, or --project --shared to install there anyway",
        );
    }
    let Some(home) = std::env::home_dir() else {
        return out.error(Exit::Precondition, "could not find the home directory");
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            return out.error(
                Exit::Precondition,
                &format!("reading the current directory: {e}"),
            );
        }
    };
    let project = project_root(&cwd, harness, scope);
    let config = harness.config_path(scope, &home, &project);
    let skill = harness.skill_path(scope, &home, &project);
    let format = match harness {
        Harness::Claude => Format::Claude,
        Harness::Codex => Format::Codex,
    };

    let mut files = Vec::new();
    let mut notes = Vec::new();
    // At project scope, the other kind of project install may be there too,
    // from an earlier setup: uninstalling removes both, and installing says
    // when the other's hooks are not these, since both would run.
    let other_scope = match (harness, scope) {
        (Harness::Codex, _) | (_, Scope::Global) => None,
        (Harness::Claude, Scope::Local) => Some(Scope::Shared),
        (Harness::Claude, Scope::Shared) => Some(Scope::Local),
    };
    let (other_config, other_skill) = match other_scope {
        Some(other) => {
            let root = project_root(&cwd, harness, other);
            let existing = |path: PathBuf| Some(path).filter(|p| p.exists());
            (
                existing(harness.config_path(other, &home, &root)).filter(|p| *p != config),
                existing(harness.skill_path(other, &home, &root)).filter(|p| *p != skill),
            )
        }
        None => (None, None),
    };
    match plan_config(&config, harness, args.uninstall) {
        Ok(change) => files.push(change),
        Err(message) => return out.error(Exit::Precondition, &message),
    }
    if let Some(other) = other_config {
        if args.uninstall {
            match plan_config(&other, harness, true) {
                Ok(change) => files.push(change),
                Err(message) => return out.error(Exit::Precondition, &message),
            }
        } else if read_hooks(&other).is_ok_and(|(_, doc)| outdated(&doc, harness)) {
            notes.push(format!(
                "{} has jouet's hooks from another version, which would run as well as these; \
                 `jouet setup {} --project --uninstall` removes them from both files",
                other.display(),
                harness.name()
            ));
        }
    }
    if !args.uninstall {
        notes.extend(hk_stop_notes(harness, &config, &home, &cwd));
    }
    let skill_text = (!args.uninstall).then(|| stamp(&skill::render(format)));
    match plan_skill(&skill, skill_text) {
        Ok((change, note)) => {
            files.push(change);
            notes.extend(note);
        }
        Err(message) => return out.error(Exit::Precondition, &message),
    }

    if args.uninstall
        && let Some(other) = other_skill
    {
        match plan_skill(&other, None) {
            Ok((change, note)) => {
                files.push(change);
                notes.extend(note);
            }
            Err(message) => return out.error(Exit::Precondition, &message),
        }
    }

    if !args.dry_run {
        for file in &files {
            if let Err(e) = apply(file) {
                return out.error(Exit::Failed, &format!("writing {}: {e}", file.path));
            }
        }
    }
    let changed = files
        .iter()
        .any(|f| matches!(f.action, Action::Create | Action::Update | Action::Remove));
    if !args.uninstall {
        notes.extend(install_notes(
            harness,
            scope,
            &project,
            &[&config, &skill],
            changed,
        ));
    }
    let report = Report {
        harness: harness.name(),
        scope: match scope {
            Scope::Global => "global",
            Scope::Local | Scope::Shared => "project",
        },
        shared: scope == Scope::Shared,
        dry_run: args.dry_run,
        uninstall: args.uninstall,
        files,
        notes,
    };
    out.result(&report, || text(&report));
    Exit::Ok
}

/// Where hk's stop hook is: merged into jouet's in `config`, or in another
/// of the harness's hook files, where it still blocks on its own.
fn hk_stop_notes(harness: Harness, config: &Path, home: &Path, cwd: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    let has = |path: &Path| read_hooks(path).is_ok_and(|(_, doc)| setup::has_hk_stop(&doc));
    if has(config) {
        notes.push(format!(
            "hk's stop hook in {} now runs inside jouet's, so a stop gets one verdict; \
             `jouet setup {} --uninstall` puts it back",
            config.display(),
            harness.name()
        ));
    }
    let mut others: Vec<PathBuf> = Vec::new();
    for scope in [Scope::Global, Scope::Local, Scope::Shared] {
        let path = harness.config_path(scope, home, &project_root(cwd, harness, scope));
        if path != config && !others.contains(&path) && has(&path) {
            others.push(path);
        }
    }
    for path in others {
        notes.push(format!(
            "{} has hk's stop hook too: it runs beside jouet's and can block on its own; \
             run `jouet setup` for that file to merge them",
            path.display()
        ));
    }
    notes
}

/// Whether a hook file holds jouet hooks other than the ones this version
/// installs. Claude Code runs a handler defined in two settings files once,
/// so only different ones would run twice.
fn outdated(doc: &serde_json::Value, harness: Harness) -> bool {
    let mut current = doc.clone();
    setup::install(&mut current, harness);
    has_jouet(doc) && current != *doc
}

/// What someone installing should know: that the hooks are inert without
/// jouet on `PATH`, who else gets them, what git would pick up, and Codex's
/// trust step.
fn install_notes(
    harness: Harness,
    scope: Scope,
    project: &Path,
    paths: &[&Path],
    changed: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if !on_path("jouet") {
        notes.push(
            "jouet is not on PATH here, so the hooks will do nothing until it is".to_string(),
        );
    }
    match scope {
        Scope::Global => {}
        Scope::Shared => notes.push(
            "everyone who clones this repository gets these hooks: they run where jouet is on PATH \
             and do nothing elsewhere"
                .to_string(),
        ),
        Scope::Local => {
            let unignored: Vec<String> = paths
                .iter()
                .filter_map(|path| {
                    let relative = path.strip_prefix(project).ok()?.to_str()?;
                    (git::ignored(project, relative) == Some(false)).then(|| relative.to_string())
                })
                .collect();
            if !unignored.is_empty() {
                let it = if unignored.len() == 1 { "it" } else { "them" };
                notes.push(format!(
                    "git does not ignore {}, so `git add` could commit {it}: \
                     list {it} in .gitignore, or in .git/info/exclude for this clone alone",
                    unignored.join(" or ")
                ));
            }
        }
    }
    if harness == Harness::Codex && changed {
        notes.push(
            "Codex skips new or changed hooks until they are trusted: run /hooks in Codex and trust jouet's."
                .to_string(),
        );
    }
    notes
}

/// Whether `program` is on `PATH`, as the hooks' `command -v` would find it.
fn on_path(program: &str) -> bool {
    let names = if cfg!(windows) {
        vec![format!("{program}.exe"), program.to_string()]
    } else {
        vec![program.to_string()]
    };
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
    })
}

/// Where a project install goes: the repository root above `cwd`, or `cwd`
/// itself outside a repository. Claude Code reads `settings.local.json` from
/// the main checkout's root even in a linked worktree (except on Windows), so
/// your own files go there.
fn project_root(cwd: &Path, harness: Harness, scope: Scope) -> PathBuf {
    if harness == Harness::Claude
        && scope == Scope::Local
        && !cfg!(windows)
        && let Some(main) = git::main_checkout()
    {
        return main;
    }
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

fn plan_config(path: &Path, harness: Harness, uninstall: bool) -> Result<FileChange, String> {
    let (before, mut doc) = read_hooks(path)?;
    let original = doc.clone();
    if uninstall {
        setup::uninstall(&mut doc);
    } else {
        setup::install(&mut doc, harness);
    }
    if doc == original {
        // Same JSON: leave the file, and its formatting, alone.
        return plan_file(path, before);
    }
    // A file uninstalling empties is kept as `{}`: it may have been there
    // before jouet was.
    let mut text = serde_json::to_string_pretty(&doc).expect("JSON values serialize");
    text.push('\n');
    plan_file(path, Some(text))
}

/// A hook file's text, if it exists, and its document, refused unless it has
/// the shape a hook file must have.
fn read_hooks(path: &Path) -> Result<(Option<String>, serde_json::Value), String> {
    let before = read(path)?;
    let doc = match before.as_deref().map(str::trim) {
        None | Some("") => serde_json::json!({}),
        Some(text) => serde_json::from_str(text).map_err(|e| {
            format!(
                "{} is not valid JSON ({e}); fix it or move it aside, then run this again",
                path.display()
            )
        })?,
    };
    setup::validate(&doc).map_err(|e| {
        format!(
            "{} does not look like a hooks file: {e}; nothing was changed",
            path.display()
        )
    })?;
    Ok((before, doc))
}

/// What writing `after` to `path` (or removing it, for `None`) would do.
fn plan_file(path: &Path, after: Option<String>) -> Result<FileChange, String> {
    let before = read(path)?;
    let action = match (&before, &after) {
        (None, None) => Action::Unchanged,
        (Some(b), Some(a)) if b == a => Action::Unchanged,
        (None, Some(_)) => Action::Create,
        (Some(_), Some(_)) => Action::Update,
        (Some(_), None) => Action::Remove,
    };
    let diff = if action == Action::Unchanged {
        String::new()
    } else {
        let name = path.display().to_string();
        similar::TextDiff::from_lines(
            before.as_deref().unwrap_or(""),
            after.as_deref().unwrap_or(""),
        )
        .unified_diff()
        .context_radius(3)
        .header(&name, &name)
        .to_string()
    };
    Ok(FileChange {
        path: path.display().to_string(),
        action,
        diff,
        contents: after,
    })
}

const STAMP: &str = "<!-- written by jouet setup, sha256:";

/// The skill with a last line recording a hash of everything above it, so a
/// later setup can tell its own unedited file from anyone else's.
fn stamp(text: &str) -> String {
    format!(
        "{text}\n{STAMP}{} (jouet setup leaves this file alone once it is edited) -->\n",
        sha256(text)
    )
}

/// Whether setup wrote this skill file and nobody has edited it since.
fn stamped_by_jouet(text: &str) -> bool {
    let body = text.trim_end_matches('\n');
    let Some((above, last)) = body.rsplit_once('\n') else {
        return false;
    };
    let Some(hash) = last.strip_prefix(STAMP).and_then(|r| r.split(' ').next()) else {
        return false;
    };
    sha256(above) == hash
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Like [`plan_file`], but a skill file setup did not write, or one edited
/// since, is kept as it is.
fn plan_skill(path: &Path, after: Option<String>) -> Result<(FileChange, Option<String>), String> {
    match read(path)? {
        Some(existing) if !stamped_by_jouet(&existing) => {
            let doing = if after.is_some() {
                "install jouet's"
            } else {
                "remove it"
            };
            let change = FileChange {
                path: path.display().to_string(),
                action: Action::Keep,
                diff: String::new(),
                contents: None,
            };
            let note = format!(
                "kept {}: jouet setup did not write it, or it was edited since; move it aside to {doing}",
                path.display()
            );
            Ok((change, Some(note)))
        }
        _ => Ok((plan_file(path, after)?, None)),
    }
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

fn apply(file: &FileChange) -> std::io::Result<()> {
    let path = Path::new(&file.path);
    match (file.action, &file.contents) {
        (Action::Create | Action::Update, Some(contents)) => {
            // Through a symlink (dotfile managers link these files) to the
            // file it names, keeping that file's permissions.
            let target = resolve_links(path)?;
            let dir = target.parent().unwrap_or(Path::new("."));
            std::fs::create_dir_all(dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            std::io::Write::write_all(&mut tmp, contents.as_bytes())?;
            if let Ok(meta) = std::fs::metadata(&target) {
                tmp.as_file().set_permissions(meta.permissions())?;
            }
            tmp.persist(&target).map_err(|e| e.error)?;
        }
        (Action::Remove, _) => {
            std::fs::remove_file(path)?;
            // The skill's own directory, if that left it empty.
            if path.file_name().is_some_and(|n| n == "SKILL.md")
                && let Some(dir) = path.parent()
            {
                let _ = std::fs::remove_dir(dir);
            }
        }
        _ => {}
    }
    Ok(())
}

/// The file `path` names after following symlinks, even a dangling one.
fn resolve_links(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..16 {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = std::fs::read_link(&current)?;
                current = match current.parent() {
                    Some(dir) if link.is_relative() => dir.join(link),
                    _ => link,
                };
            }
            _ => return Ok(current),
        }
    }
    Err(std::io::Error::other(format!(
        "{} is a chain of more than 16 symlinks",
        path.display()
    )))
}

fn text(report: &Report) -> String {
    let mut out = String::new();
    for file in &report.files {
        let verb = match (file.action, report.dry_run) {
            (Action::Create, false) => "created",
            (Action::Create, true) => "would create",
            (Action::Update, false) => "updated",
            (Action::Update, true) => "would update",
            (Action::Remove, false) => "removed",
            (Action::Remove, true) => "would remove",
            (Action::Unchanged, _) => "unchanged",
            (Action::Keep, _) => "kept",
        };
        let _ = writeln!(out, "{verb} {}", file.path);
        if report.dry_run {
            out.push_str(&file.diff);
        }
    }
    for note in &report.notes {
        let _ = writeln!(out, "note: {note}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unedited_stamped_skill_is_setups_own() {
        let skill = stamp("---\nname: jouet\n---\n\nbody\n");
        assert!(stamped_by_jouet(&skill));
        assert!(!stamped_by_jouet(&skill.replace("body", "my notes")));
        assert!(!stamped_by_jouet("---\nname: jouet\n---\n\nmy own skill\n"));
        assert!(!stamped_by_jouet(""));
    }
}
