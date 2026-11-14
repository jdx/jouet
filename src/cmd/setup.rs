//! `jouet setup HARNESS`: installs jouet's hooks and skill.

use super::Output;
use super::skill::{self, Format};
use crate::budget;
use crate::cli::SetupArgs;
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
    /// A link to make, Claude Code's skill directory naming the shared one.
    Link,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    benchmark_prompt: Option<&'static str>,
    /// Claude Code: the context budget a plain setup applied (or, with
    /// `--dry-run`, would apply), from your recent sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    context_budget: Option<offer::Offer>,
}

pub(crate) fn run(args: &SetupArgs, out: &Output) -> Exit {
    let Some(harness) = Harness::parse(&args.harness) else {
        return out.error(
            Exit::Usage,
            &format!(
                "the harness must be {}, not `{}`",
                setup::NAMES,
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
    if args.benchmark && (args.dry_run || args.uninstall) {
        return out.error(
            Exit::Usage,
            "--benchmark cannot be combined with --dry-run or --uninstall",
        );
    }
    let scope = if args.shared {
        Scope::Shared
    } else if args.project {
        Scope::Local
    } else {
        Scope::Global
    };
    let budget_flags = args.compact_window.is_some() || args.deny_unused || args.undo.is_some();
    if budget_flags && (harness != Harness::Claude || scope == Scope::Shared) {
        return out.error(
            Exit::Usage,
            "--compact-window, --deny-unused and --undo change Claude Code's settings for you alone: \
             use them with `jouet setup claude`, without --shared",
        );
    }
    if budget_flags && args.uninstall {
        return out.error(
            Exit::Usage,
            "--uninstall already takes back what --compact-window and --deny-unused added",
        );
    }
    let window = match args.compact_window.as_deref().map(budget::parse_window) {
        Some(Err(message)) => return out.error(Exit::Usage, &message),
        Some(Ok(window)) => Some(window),
        None => None,
    };
    if !harness.has_local_file() && scope == Scope::Local && !args.uninstall {
        let file = harness.config_path(Scope::Shared, Path::new(""), Path::new(""));
        return out.error(
            Exit::Usage,
            &format!(
                "{} has no project hooks file of your own: {} is the repository's, \
                 shared with everyone who clones it. Use --global, or --project --shared to install there anyway",
                harness.title(),
                file.display()
            ),
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
    let skill = harness.skill_path(scope, &home, &project, "jouet");
    let init_skill = harness.skill_path(scope, &home, &project, "jouet-init");
    // Every harness but Claude Code reads the shared `.agents/skills`, in
    // Codex's shape.
    let format = match harness {
        Harness::Claude => Format::Claude,
        _ => Format::Codex,
    };

    let mut files = Vec::new();
    let mut notes = Vec::new();
    // At project scope, the other kind of project install may be there too,
    // from an earlier setup: uninstalling removes both, and installing says
    // when the other's hooks are not these, since both would run.
    let other_scope = match scope {
        _ if !harness.has_local_file() => None,
        Scope::Global => None,
        Scope::Local => Some(Scope::Shared),
        Scope::Shared => Some(Scope::Local),
    };
    let (other_config, other_skills) = match other_scope {
        Some(other) => {
            let root = project_root(&cwd, harness, other);
            let existing = |path: PathBuf| Some(path).filter(|p| p.exists());
            (
                existing(harness.config_path(other, &home, &root)).filter(|p| *p != config),
                ["jouet", "jouet-init"]
                    .into_iter()
                    .filter_map(|name| existing(harness.skill_path(other, &home, &root, name)))
                    .filter(|p| *p != skill && *p != init_skill)
                    .collect::<Vec<_>>(),
            )
        }
        None => (None, Vec::new()),
    };
    // The context budget, for Claude Code's settings of your own: an
    // explicit flag, or by default what your recent sessions call for,
    // except a part you took back with --undo or all of it with
    // --no-context-budget.
    let setup_command = match scope {
        Scope::Local => "jouet setup claude --project",
        _ => "jouet setup claude",
    };
    let mut choice = budget::Change::default();
    let mut context_budget = None;
    if harness == Harness::Claude && scope != Scope::Shared {
        if args.uninstall {
            choice.undo_window = true;
            choice.undo_unused = true;
        } else {
            choice.window = window;
            choice.undo_window = args.undo.as_deref() == Some("window");
            choice.undo_unused = args.undo.as_deref() == Some("unused");
            choice.decline = args.undo.is_some();
            choice.deny_asked = args.deny_unused;
            if args.deny_unused || !(budget_flags || args.no_context_budget) {
                let doc = read_hooks(&config).map_or(serde_json::Value::Null, |(_, doc)| doc);
                let root = jouet_output::paths::env_dir("CLAUDE_CONFIG_DIR")
                    .unwrap_or_else(|| home.join(".claude"));
                let history = budget::scan(&root);
                let unused = budget::offered(&history, &doc);
                let record = budget::load(&config);
                // What an earlier run picked that the rule now keeps goes
                // back on every run that reads the history.
                let restore = budget::wrong_picks(&record, &history.servers);
                if args.deny_unused {
                    if unused.is_empty() {
                        notes.push(format!(
                            "nothing to deny: {}",
                            if history.sessions < budget::MIN_SESSIONS {
                                format!(
                                    "only {} recent sessions recorded their tools, and it takes {}",
                                    history.sessions,
                                    budget::MIN_SESSIONS
                                )
                            } else {
                                format!(
                                    "every MCP server and plugin skill you added was used in your last {} sessions or is already named in the settings",
                                    history.sessions
                                )
                            }
                        ));
                    }
                    choice.deny = unused;
                } else {
                    // A window that is setup's own is chosen already, so
                    // a second run sets nothing new. At project scope, a
                    // window in your global settings or the project's
                    // shared ones is a choice too, which a local window
                    // would override.
                    let elsewhere = match scope {
                        Scope::Local => [Scope::Global, Scope::Shared]
                            .into_iter()
                            .map(|other| harness.config_path(other, &home, &project))
                            .filter(|path| *path != config)
                            .any(|path| {
                                read_hooks(&path).is_ok_and(|(_, doc)| budget::window_chosen(&doc))
                            }),
                        _ => false,
                    };
                    let window = (history.past_window > 0
                        && !record.declined_window
                        && !budget::window_chosen(&doc)
                        && !elsewhere)
                        .then_some(budget::WINDOW);
                    let unused = if record.declined_unused {
                        Vec::new()
                    } else {
                        unused
                    };
                    let found = offer::Offer::new(&history, window, unused);
                    if !found.is_empty() {
                        notes.push(found.note(setup_command, args.dry_run));
                        choice.window = found.compact_window;
                        choice.deny.clone_from(&found.unused);
                        context_budget = Some(found);
                    }
                }
                choice.restore = restore;
            }
        }
    }
    choice.dry_run = args.dry_run;
    let budget_before = budget::load(&config);
    let mut records = Vec::new();
    match plan_config(&config, harness, scope, args.uninstall, &choice) {
        Ok((change, record, budget_notes)) => {
            files.push(change);
            records.extend(record.map(|r| (config.clone(), r)));
            notes.extend(budget_notes);
        }
        Err(message) => return out.error(Exit::Precondition, &message),
    }
    if let Some(other) = other_config {
        if args.uninstall {
            match plan_config(&other, harness, scope, true, &choice) {
                Ok((change, record, _)) => {
                    files.push(change);
                    records.extend(record.map(|r| (other.clone(), r)));
                }
                Err(message) => return out.error(Exit::Precondition, &message),
            }
        } else if read_hooks(&other).is_ok_and(|(_, doc)| outdated(&doc, harness, scope)) {
            notes.push(format!(
                "{} has jouet's hooks from another version, which would run as well as these; \
                 `jouet setup {} --project --uninstall` removes them from both files",
                other.display(),
                harness.name()
            ));
        }
    }
    if !args.uninstall {
        notes.extend(hk_stop_notes(harness, scope, &config, &home, &cwd));
    }
    // The primer says what the skill says, in every session, so the skill
    // is installed only when asked for or when priming is off. Without it,
    // setup's own unedited skill from an earlier install goes, and anyone
    // else's is left alone without a word.
    let hooks = crate::settings::Settings::load().hooks;
    let want_skill = !args.uninstall && (args.skill || !(hooks.enabled && hooks.prime));
    // Every harness reads the same skill text, so in a shared install Claude
    // Code's `.claude/skills/NAME` is a link to the `.agents/skills/NAME` the
    // others read, and the repository keeps one copy. A --project install is
    // yours alone and stays where git can ignore it.
    let share = harness == Harness::Claude && scope == Scope::Shared;
    // Whether a harness other than Claude Code, or Claude Code, still has
    // jouet's hooks in this project, and so reads a shared skill.
    let project_hooks = |harnesses: &[Harness]| {
        harnesses.iter().any(|h| {
            [Scope::Shared, Scope::Local].into_iter().any(|s| {
                let path = h.config_path(s, &home, &project);
                path != config && read_hooks(&path).is_ok_and(|(_, doc)| has_jouet(&doc))
            })
        })
    };
    let others_read_it = project_hooks(&[
        Harness::Codex,
        Harness::Cursor,
        Harness::Gemini,
        Harness::Copilot,
    ]);
    let claude_reads_it = project_hooks(&[Harness::Claude]);
    // What installing `text` at `path`, or removing it (`None`), changes.
    let plan_at = |path: &Path,
                   text: Option<String>|
     -> Result<Vec<(FileChange, Option<String>)>, String> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let shared = shared_skill(&project, path);
        match text {
            Some(text) if share && linkable(path) => {
                let file = plan_skill(&shared, Some(text))?;
                let already = std::fs::read_link(dir).is_ok_and(|t| t == link_target(path));
                let link = FileChange {
                    path: dir.display().to_string(),
                    action: if already {
                        Action::Unchanged
                    } else {
                        Action::Link
                    },
                    diff: String::new(),
                    contents: None,
                };
                Ok(vec![file, (link, None)])
            }
            // A shared install's link, which setup made: anyone else's link
            // reaches plan_skill, which keeps a skill jouet did not write.
            None if harness == Harness::Claude
                && scope != Scope::Global
                && std::fs::read_link(dir).is_ok_and(|t| t == link_target(path)) =>
            {
                // The link goes, and the shared file it names unless another
                // harness here still has jouet's hooks and reads it.
                let mut changes = vec![(
                    FileChange {
                        path: dir.display().to_string(),
                        action: Action::Remove,
                        diff: String::new(),
                        contents: None,
                    },
                    None,
                )];
                if !others_read_it && harness == Harness::Claude {
                    changes.push(plan_skill(&shared, None)?);
                }
                Ok(changes)
            }
            // Claude Code's copy, through a link on `.claude/skills` as a
            // whole, that another harness here still reads.
            None if harness == Harness::Claude
                && scope != Scope::Global
                && others_read_it
                && dir.parent().is_some_and(is_link)
                && claude_links_to(&project, &shared) =>
            {
                let change = FileChange {
                    path: path.display().to_string(),
                    action: Action::Keep,
                    diff: String::new(),
                    contents: None,
                };
                let note = format!(
                    "kept {}: another harness here still has jouet's hooks and reads it",
                    path.display()
                );
                Ok(vec![(change, Some(note))])
            }
            // Another harness's copy that Claude Code's link still names.
            None if harness != Harness::Claude
                && claude_reads_it
                && claude_links_to(&project, path) =>
            {
                let change = FileChange {
                    path: path.display().to_string(),
                    action: Action::Keep,
                    diff: String::new(),
                    contents: None,
                };
                let note = format!(
                    "kept {}: Claude Code's skill links to it, and Claude Code still has jouet's hooks here",
                    path.display()
                );
                Ok(vec![(change, Some(note))])
            }
            text => Ok(vec![plan_skill(path, text)?]),
        }
    };
    let mut plan = |path: &Path, text: Option<String>| -> Result<(), String> {
        for (change, note) in plan_at(path, text)? {
            files.push(change);
            notes.extend(note);
        }
        Ok(())
    };
    let ours = matches!(read(&skill), Ok(Some(text)) if stamped_by_jouet(&text));
    if want_skill || args.uninstall || ours {
        let skill_text = want_skill.then(|| stamp(&skill::render(format)));
        if let Err(message) = plan(&skill, skill_text) {
            return out.error(Exit::Precondition, &message);
        }
    }
    // jouet-init likewise, only when asked for: it ran in no real session
    // over 14 days and gained nothing over jouet's defaults on the bench. An
    // earlier setup's unedited copy goes.
    let want_init = !args.uninstall && args.init_skill;
    let init_ours = matches!(read(&init_skill), Ok(Some(text)) if stamped_by_jouet(&text));
    if want_init || args.uninstall || init_ours {
        let init_text = want_init.then(|| stamp(&skill::init(format)));
        if let Err(message) = plan(&init_skill, init_text) {
            return out.error(Exit::Precondition, &message);
        }
    }
    if args.uninstall {
        for other in other_skills {
            if let Err(message) = plan(&other, None) {
                return out.error(Exit::Precondition, &message);
            }
        }
    }

    if !args.dry_run {
        for file in &files {
            if let Err(e) = apply(file) {
                return out.error(Exit::Failed, &format!("writing {}: {e}", file.path));
            }
        }
        for (path, record) in &records {
            if let Err(e) = budget::save(path, record) {
                notes.push(format!(
                    "could not record what setup added to {} ({e}), so --undo will not find it",
                    path.display()
                ));
            }
        }
    }
    if let Some((_, after)) = records.iter().find(|(path, _)| *path == config) {
        notes.extend(budget_notes(
            &budget_before,
            after,
            &choice,
            args.dry_run,
            setup_command,
        ));
    }
    let changed = files.iter().any(|f| {
        matches!(
            f.action,
            Action::Create | Action::Update | Action::Remove | Action::Link
        )
    });
    if !args.uninstall {
        let written: Vec<&Path> = std::iter::once(config.as_path())
            .chain(want_skill.then_some(skill.as_path()))
            .chain(want_init.then_some(init_skill.as_path()))
            .collect();
        notes.extend(install_notes(
            harness, scope, &home, &project, &written, changed,
        ));
        if want_init && !args.dry_run {
            let invocation = match harness {
                Harness::Claude => "/jouet-init",
                Harness::Codex => "$jouet-init",
                _ => "Use the jouet-init skill to configure jouet for this project",
            };
            notes.push(format!(
                "next: start a new agent session in your project and ask: {invocation}"
            ));
        }
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
        benchmark_prompt: args.benchmark.then_some(benchmark::PROMPT),
        context_budget,
    };
    out.result(&report, || text(&report));
    if !out.json
        && !args.uninstall
        && !args.dry_run
        && !args.benchmark
        && changed
        && jouet_output::display::person_at_terminal()
    {
        benchmark::offer();
    }
    Exit::Ok
}

mod benchmark;
mod offer;

/// What the budget changed in the settings, and how to take each back.
fn budget_notes(
    before: &budget::Added,
    after: &budget::Added,
    change: &budget::Change,
    dry_run: bool,
    setup: &str,
) -> Vec<String> {
    let mut notes = Vec::new();
    let verb = |done: &'static str, would: &'static str| if dry_run { would } else { done };
    if let Some(window) = after.auto_compact_window
        && before.auto_compact_window != Some(window)
    {
        notes.push(format!(
            "{} autoCompactWindow to {window}; `{setup} --undo window` takes it back",
            verb("set", "would set")
        ));
    }
    // Counted from what this run picked: a run that also puts earlier
    // picks back can shrink the record while it adds.
    let new: Vec<&budget::Unused> = change
        .deny
        .iter()
        .filter(|u| u.recorded_in(after) && !u.recorded_in(before))
        .collect();
    let new_skills = new.iter().filter(|u| u.kind == budget::Kind::Skill).count();
    let new_denials = new.len() - new_skills;
    if !new.is_empty() {
        let tokens: u64 = new.iter().map(|u| u.tokens).sum();
        notes.push(format!(
            "{} {new_denials} unused MCP servers and {} {new_skills} unused skills from the model, \
             ~{} tokens a request; `{setup} --undo unused` takes them back",
            verb("denied", "would deny"),
            verb("hid", "would hide"),
            budget::tokens(tokens)
        ));
    }
    if change.undo_unused && !(before.deny.is_empty() && before.skills.is_empty()) {
        notes.push(format!(
            "{} the {} denials and {} skill overrides setup added",
            verb("removed", "would remove"),
            before.deny.len(),
            before.skills.len()
        ));
    }
    notes
}

/// Where hk's stop hook is: merged into jouet's in `config`, or in another
/// of the harness's hook files, where it still blocks on its own.
fn hk_stop_notes(
    harness: Harness,
    scope: Scope,
    config: &Path,
    home: &Path,
    cwd: &Path,
) -> Vec<String> {
    let mut notes = Vec::new();
    let has = |path: &Path| read_hooks(path).is_ok_and(|(_, doc)| setup::has_hk_stop(&doc));
    // The setup command for a scope's file.
    let setup = |scope: Scope| {
        let flags = match scope {
            Scope::Global => "",
            Scope::Local => " --project",
            Scope::Shared => " --project --shared",
        };
        format!("jouet setup {}{flags}", harness.name())
    };
    if has(config) {
        notes.push(format!(
            "hk's stop hook in {} now runs inside jouet's, so a stop gets one verdict; \
             `{} --uninstall` puts it back",
            config.display(),
            setup(scope)
        ));
    }
    let mut others: Vec<(PathBuf, Scope)> = Vec::new();
    for other in [Scope::Global, Scope::Local, Scope::Shared] {
        let path = harness.config_path(other, home, &project_root(cwd, harness, other));
        if path != config && !others.iter().any(|(p, _)| *p == path) && has(&path) {
            others.push((path, other));
        }
    }
    for (path, other) in others {
        notes.push(format!(
            "{} has hk's stop hook too: it runs beside jouet's and can block on its own; \
             `{}` merges them",
            path.display(),
            setup(other)
        ));
    }
    notes
}

/// Whether a hook file holds jouet hooks other than the ones this version
/// installs. Claude Code runs a handler defined in two settings files once,
/// so only different ones would run twice.
fn outdated(doc: &serde_json::Value, harness: Harness, scope: Scope) -> bool {
    let mut current = doc.clone();
    setup::install(&mut current, harness, scope);
    has_jouet(doc) && current != *doc
}

/// What someone installing should know: that the hooks are inert without
/// jouet on `PATH`, who else gets them, what git would pick up, each
/// harness's trust step, and in a fnox project, its credential proxy.
fn install_notes(
    harness: Harness,
    scope: Scope,
    home: &Path,
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
    match harness {
        Harness::Codex if changed => notes.push(
            "Codex skips new or changed hooks until they are trusted: run /hooks in Codex and trust jouet's."
                .to_string(),
        ),
        Harness::Cursor => {
            if scope != Scope::Global {
                notes.push(
                    "Cursor runs a project's hooks only once the workspace is trusted".to_string(),
                );
            }
            let claude = [
                Harness::Claude.config_path(Scope::Global, home, project),
                project.join(".claude").join("settings.json"),
                project.join(".claude").join("settings.local.json"),
            ];
            for path in claude {
                if read_hooks(&path).is_ok_and(|(_, doc)| has_jouet(&doc)) {
                    notes.push(format!(
                        "Cursor also runs the Claude Code hooks in {}: while these are installed, \
                         those step aside in Cursor sessions, so each call gets one verdict",
                        path.display()
                    ));
                }
            }
        }
        Harness::Gemini => notes.push(
            "Gemini CLI runs hooks from settings files only in a trusted folder".to_string(),
        ),
        Harness::Copilot => {
            notes.push(
                "Copilot reads hooks when it starts, and a repository's only once you trust the folder: \
                 restart Copilot CLI to pick these up"
                    .to_string(),
            );
            if scope == Scope::Shared {
                notes.push(
                    "Copilot's cloud agent uses .github/hooks/ only from the default branch, and \
                     does nothing with them where its runner has no jouet on PATH"
                        .to_string(),
                );
            }
        }
        _ => {}
    }
    notes.extend(fnox_note(harness, project, crate::fnox::under_proxy()));
    notes
}

/// In a project that uses fnox, a recommendation to start the agent under
/// `fnox proxy run`.
fn fnox_note(harness: Harness, project: &Path, proxied: bool) -> Option<String> {
    crate::fnox::Project::find(project)?;
    let agent = match harness {
        Harness::Cursor => "cursor-agent",
        _ => harness.name(),
    };
    Some(if proxied {
        "this repository uses fnox, and this shell already runs under `fnox proxy run`: an agent started from here gets placeholder credentials"
            .to_string()
    } else {
        format!(
            "this repository uses fnox: start the agent with `fnox proxy run -- {agent}` so it holds only placeholder credentials, and fnox puts the real values into the HTTPS requests its [[proxy.rules]] allow (https://fnox.jdx.dev/guide/proxy)"
        )
    })
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

/// The hooks file's change, and for Claude Code what the budget has added to
/// it afterwards, with notes on what the budget left alone.
fn plan_config(
    path: &Path,
    harness: Harness,
    scope: Scope,
    uninstall: bool,
    budget: &budget::Change,
) -> Result<(FileChange, Option<budget::Added>, Vec<String>), String> {
    let (before, mut doc) = read_hooks(path)?;
    let original = doc.clone();
    if uninstall {
        setup::uninstall(&mut doc);
    } else {
        setup::install(&mut doc, harness, scope);
    }
    let mut record = None;
    let mut notes = Vec::new();
    if harness == Harness::Claude && !budget.is_empty() {
        let mut added = budget::load(path);
        notes = budget::apply(&mut doc, budget, &mut added);
        record = Some(added);
    }
    if doc == original {
        // Same JSON: leave the file, and its formatting, alone.
        return Ok((plan_file(path, before)?, record, notes));
    }
    // jouet's own file goes once nothing else is in it.
    if harness.owns_file() && setup::is_empty(&doc) {
        return Ok((plan_file(path, None)?, record, notes));
    }
    // A file uninstalling empties is kept as `{}`: it may have been there
    // before jouet was.
    let mut text = serde_json::to_string_pretty(&doc).expect("JSON values serialize");
    text.push('\n');
    Ok((plan_file(path, Some(text))?, record, notes))
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

/// The shared copy of the skill Claude Code keeps at `skill`
/// (`.claude/skills/NAME/SKILL.md`): `.agents/skills/NAME/SKILL.md`.
fn shared_skill(project: &Path, skill: &Path) -> PathBuf {
    let name = skill.parent().and_then(Path::file_name).unwrap_or_default();
    project
        .join(".agents")
        .join("skills")
        .join(name)
        .join("SKILL.md")
}

/// What Claude Code's skill directory for `skill` links to, relative to it.
fn link_target(skill: &Path) -> PathBuf {
    let name = skill.parent().and_then(Path::file_name).unwrap_or_default();
    Path::new("..")
        .join("..")
        .join(".agents")
        .join("skills")
        .join(name)
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Whether Claude Code's skill directory for `skill` can be the link: it is
/// not there, or it is a link (jouet's, or one left dangling), or it is a
/// directory an earlier setup made, holding only its own unedited skill.
/// Not when `.claude/skills` is itself a link, as in a repository that
/// shares the whole directory already: the file reaches `.agents` through it.
fn linkable(skill: &Path) -> bool {
    let Some(dir) = skill.parent() else {
        return false;
    };
    if dir.parent().is_some_and(is_link) {
        return false;
    }
    match std::fs::symlink_metadata(dir) {
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        // jouet's own link, or one whose target is gone; never a link of
        // someone's to a skill of their own.
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::read_link(dir).is_ok_and(|t| t == link_target(skill)) || !dir.exists()
        }
        Ok(meta) if meta.is_dir() => {
            std::fs::read_dir(dir).is_ok_and(|entries| entries.count() == 1)
                && matches!(read(skill), Ok(Some(text)) if stamped_by_jouet(&text))
        }
        Ok(_) => false,
    }
}

/// Whether Claude Code's skill directory in `project` for the skill at the
/// shared `path` is that directory through a link: its own, or one on
/// `.claude/skills` as a whole.
fn claude_links_to(project: &Path, path: &Path) -> bool {
    let Some(name) = path.parent().and_then(Path::file_name) else {
        return false;
    };
    let dir = project.join(".claude").join("skills").join(name);
    let resolved = |p: &Path| std::fs::canonicalize(p).ok();
    resolved(&dir).is_some() && resolved(&dir) == path.parent().and_then(resolved)
}

/// Makes Claude Code's skill directory `dir` the link to the shared copy,
/// which setup has just written, replacing what [`linkable`] allowed there.
/// Where links cannot be made (Windows without the right), the directory
/// gets a copy of the shared file instead.
fn make_link(dir: &Path) -> std::io::Result<()> {
    let skill = dir.join("SKILL.md");
    let target = link_target(&skill);
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(dir).or_else(|_| std::fs::remove_dir(dir))?;
        }
        Ok(_) => {
            std::fs::remove_file(&skill)?;
            std::fs::remove_dir(dir)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&target, dir);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&target, dir);
    if linked.is_err() {
        std::fs::create_dir_all(dir)?;
        // A relative link resolves from the directory it is in.
        let base = dir.parent().unwrap_or(Path::new("."));
        std::fs::copy(base.join(&target).join("SKILL.md"), &skill)?;
    }
    Ok(())
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
        (Action::Link, _) => make_link(path)?,
        // A skill directory's link: removed itself, not what it names.
        (Action::Remove, _)
            if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) =>
        {
            std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))?;
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
            (Action::Link, false) => "linked",
            (Action::Link, true) => "would link",
        };
        let _ = writeln!(out, "{verb} {}", file.path);
        if report.dry_run {
            out.push_str(&file.diff);
        }
    }
    for note in &report.notes {
        let _ = writeln!(out, "note: {note}");
    }
    if let Some(prompt) = report.benchmark_prompt {
        benchmark::write_prompt(&mut out, prompt);
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

    #[test]
    fn a_fnox_project_is_pointed_at_the_proxy() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fnox_note(Harness::Claude, dir.path(), false), None);
        std::fs::write(dir.path().join("fnox.toml"), "[secrets]\n").unwrap();
        assert!(
            fnox_note(Harness::Codex, dir.path(), false)
                .unwrap()
                .contains("`fnox proxy run -- codex`")
        );
        assert!(
            fnox_note(Harness::Claude, dir.path(), true)
                .unwrap()
                .contains("already runs under `fnox proxy run`")
        );
    }
}
