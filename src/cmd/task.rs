//! `jouet task propose` and `jouet task apply`: drafting mise tasks for the
//! commands agents repeat in a repository, and adding one through mise.

use super::Output;
use crate::mise::{self, Problem};
use crate::settings::Settings;
use crate::wind::tasks::{self, Applied, Draft, File, Project, Recorded};
use crate::wind::{self, Paths, Status};
use crate::{TaskApplyArgs, TaskArgs, TaskCmd, TaskProposeArgs};
use jouet_output::exit::Exit;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Sessions read unless --since says otherwise: the last two weeks'.
const SINCE: &str = "14d";

/// Where a repository keeps a mise config it shares, in the order mise
/// reads them.
const SHARED_CONFIGS: &[&str] = &[
    "mise.toml",
    ".mise.toml",
    "mise/config.toml",
    ".mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
];

/// Files that say a program is part of the project's own workflow.
const MANIFESTS: &[(&str, &[&str])] = &[
    ("Cargo.toml", &["cargo", "rustc"]),
    (
        "package.json",
        &["npm", "npx", "node", "pnpm", "yarn", "bun"],
    ),
    ("go.mod", &["go"]),
    (
        "pyproject.toml",
        &["python", "python3", "pytest", "uv", "ruff"],
    ),
    ("Makefile", &["make"]),
    ("justfile", &["just"]),
    ("hk.pkl", &["hk"]),
    ("Gemfile", &["bundle", "rake", "ruby"]),
    ("gradlew", &["gradlew"]),
];

/// Where a repository's CI, docs and build files say what it runs.
const DOCUMENTED_IN: &[&str] = &[
    ".github",
    "CONTRIBUTING.md",
    "README.md",
    "docs",
    "Makefile",
    "justfile",
    "hk.pkl",
    "package.json",
];

pub(crate) fn run(args: &TaskArgs, out: &Output) -> Exit {
    let Some(paths) = Paths::locate() else {
        return out.error(
            Exit::Failed,
            "there is no home directory to keep winding's state in",
        );
    };
    match &args.cmd {
        TaskCmd::Propose(args) => propose(&paths, args, out),
        TaskCmd::Apply(args) => apply(&paths, args, out),
    }
}

/// The work tree jouet runs in, or the current directory outside one.
fn root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    jouet_output::paths::work_tree(&cwd).unwrap_or(cwd)
}

fn tracked(root: &Path, path: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", path])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The mise config file the repository shares, relative to its root.
pub(crate) fn shared_config(root: &Path) -> Option<&'static str> {
    SHARED_CONFIGS
        .iter()
        .copied()
        .find(|name| root.join(name).is_file() && tracked(root, name))
}

/// The programs the project's manifests and shared mise tools name.
fn tools(root: &Path, config: Option<&str>) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = MANIFESTS
        .iter()
        .filter(|(file, _)| root.join(file).exists())
        .flat_map(|(_, programs)| programs.iter().map(|p| (*p).to_string()))
        .collect();
    let table = config
        .and_then(|c| std::fs::read_to_string(root.join(c)).ok())
        .and_then(|text| text.parse::<toml::Table>().ok());
    let Some(table) = table else {
        return out;
    };
    let names = table
        .get("tools")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|t| t.keys());
    for key in names {
        let name = key.rsplit(':').next().unwrap_or(key);
        let name = name.rsplit('/').next().unwrap_or(name);
        let name = name.split('@').next().unwrap_or(name);
        match name {
            "rust" => out.extend(["cargo".into(), "rustc".into()]),
            "node" => out.extend(["node".into(), "npm".into(), "npx".into()]),
            "python" => out.extend(["python".into(), "python3".into(), "pip".into()]),
            other => {
                out.insert(other.to_string());
            }
        }
    }
    out
}

/// Whether the repository's CI, docs or build files hold `text`.
fn documented(root: &Path, text: &str) -> bool {
    if text.len() < 8 {
        return false;
    }
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["grep", "--quiet", "--fixed-strings", "-e", text, "--"])
        .args(DOCUMENTED_IN)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A pending proposal as `propose` prints it.
#[derive(Serialize)]
struct Shown<'a> {
    id: &'a str,
    #[serde(flatten)]
    task: &'a Draft,
    /// What to ask the user, with jouet's suggestion.
    question: String,
    /// The command for the suggested file.
    apply: String,
}

impl<'a> Shown<'a> {
    fn of(id: &'a str, task: &'a Draft) -> Self {
        Self {
            id,
            task,
            question: tasks::question(task),
            apply: format!("jouet task apply {id} --file {}", task.suggest.file.name()),
        }
    }
}

#[derive(Serialize)]
struct ProposeReport<'a> {
    root: &'a str,
    /// Sessions whose transcripts were read.
    sessions: usize,
    /// Sessions written since --since but left out, older than those read.
    #[serde(skip_serializing_if = "is_zero")]
    left_out: usize,
    #[serde(flatten)]
    recorded: &'a Recorded,
    /// Every pending proposal for this repository.
    proposals: Vec<Shown<'a>>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn propose(paths: &Paths, args: &TaskProposeArgs, out: &Output) -> Exit {
    use wind::session::{self, Redactor, Select};
    let since_text = args.since.as_deref().unwrap_or(SINCE);
    let since = match super::audit::parse_since(since_text, jouet_output::time::now()) {
        Ok(secs) => secs.saturating_mul(1000),
        Err(e) => return out.error(Exit::Usage, &format!("--since: {e}")),
    };
    let readers = match session::Readers::from_env(args.harness.as_deref()) {
        Ok(r) => r,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let root = root();
    let root_text = root.to_string_lossy().into_owned();
    let repo = crate::ledger::repo_key(&root);
    let existing = match mise::tasks(&root) {
        Ok(tasks) => tasks,
        Err(problem) => {
            return out.error(
                Exit::Precondition,
                &format!(
                    "reading the project's tasks, to propose none it has: {}",
                    problem.message()
                ),
            );
        }
    };
    let (found, left_out) = match session::find(&readers, &Select::Since(since), Some(&root)) {
        Ok(found) => found,
        Err(e) => return out.error(Exit::NotFound, &e),
    };
    let redactor = Redactor::new(Settings::load().wind.gitleaks.as_deref());
    let mut runs = Vec::new();
    let mut sessions = 0;
    for f in &found {
        let Some(found_runs) = session::shell_runs(f, Some(&root)) else {
            continue;
        };
        sessions += 1;
        // A command with a secret in it is never proposed. The project's
        // own directory is no secret, however key-like a worktree's name
        // looks, so it is made relative first.
        runs.extend(found_runs.into_iter().filter(|r| {
            let command = tasks::normalize(&r.command, Some(&root_text));
            redactor.redact(&command) == command
        }));
    }
    let mut ledger: BTreeMap<String, u64> = BTreeMap::new();
    for entry in wind::reader::read_all(&paths.ledger) {
        if entry.event == "post-tool-use"
            && entry.repo.is_some()
            && entry.repo == repo
            && let Some(shape) = &entry.shape
        {
            *ledger.entry(tasks::shape_key(shape)).or_default() += 1;
        }
    }
    let config = shared_config(&root);
    let documented = |text: &str| documented(&root, text);
    let project = Project {
        root: &root_text,
        shared_config: config.is_some(),
        tools: tools(&root, config),
        documented: &documented,
    };
    let state = wind::State::load(&paths.state);
    let taken: BTreeSet<String> = state
        .tasks
        .proposals
        .iter()
        .filter(|p| p.repo == repo)
        .filter_map(|p| p.task.as_ref().map(|t| t.name.clone()))
        .collect();
    let drafts = tasks::drafts(&runs, &existing, &project, &ledger, &taken);
    let recorded = match wind::update(&paths.state, |state| {
        state
            .tasks
            .record(repo.as_deref(), &root_text, drafts, wind::now())
    }) {
        Ok(recorded) => recorded,
        Err(e) => return out.error(Exit::Failed, &format!("writing the winding state: {e}")),
    };
    let state = wind::State::load(&paths.state);
    let proposals: Vec<Shown> = state
        .tasks
        .proposals
        .iter()
        .filter(|p| p.repo == repo && p.status == Status::Pending)
        .filter_map(|p| p.task.as_ref().map(|t| Shown::of(&p.id, t)))
        .collect();
    let report = ProposeReport {
        root: &root_text,
        sessions,
        left_out,
        recorded: &recorded,
        proposals,
    };
    out.result(&report, || {
        let mut text = format!("read {sessions} sessions in {root_text} since {since_text}\n");
        if left_out > 0 {
            let _ = writeln!(
                text,
                "… {left_out} older sessions since then left out; narrow --since"
            );
        }
        for id in &recorded.lapsed {
            let _ = writeln!(
                text,
                "{id} lapsed: no session ran one command often enough; it varies from run to run"
            );
        }
        if report.proposals.is_empty() {
            text.push_str("no command repeated in them that a mise task would replace\n");
        }
        for p in &report.proposals {
            write_shown(&mut text, p);
        }
        text
    });
    Exit::Ok
}

fn write_shown(text: &mut String, p: &Shown<'_>) {
    let t = p.task;
    let e = &t.evidence;
    let mut seen = format!("ran {} times in {} sessions", e.runs, e.sessions);
    if e.failed > 0 {
        let _ = write!(seen, " ({} failed)", e.failed);
    }
    if e.duration_ms > 0 {
        let _ = write!(seen, ", {}", jouet_audit::digest::duration(e.duration_ms));
    }
    if e.output_tokens > 0 {
        let _ = write!(seen, ", about {} tokens of output", e.output_tokens);
    }
    if e.ledger_runs > e.runs {
        let _ = write!(seen, "; {} runs of its shape in the ledger", e.ledger_runs);
    }
    let _ = writeln!(text, "{}  {}  {seen}", p.id, t.name);
    let _ = writeln!(text, "    run      {}", t.run);
    if !t.depends.is_empty() {
        let _ = writeln!(text, "    depends  {}", t.depends.join(", "));
    }
    if !t.sources.is_empty() {
        let _ = writeln!(text, "    sources  {}", t.sources.join(" "));
    }
    let _ = writeln!(
        text,
        "    suggest  {}: {}",
        t.suggest.file.config(),
        t.suggest.reasons.join("; ")
    );
    let _ = writeln!(text, "    ask      {}", p.question);
    let _ = writeln!(
        text,
        "    apply    {}  (or --file {})",
        p.apply,
        t.suggest.file.other().name()
    );
}

#[derive(Serialize)]
struct ApplyReport<'a> {
    proposal: &'a str,
    task: &'a str,
    file: File,
    mise_version: String,
    /// The mise command jouet ran.
    command: Vec<String>,
    /// What mise reported: the file, the action, the task's table.
    mise: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

fn apply(paths: &Paths, args: &TaskApplyArgs, out: &Output) -> Exit {
    let state = wind::State::load(&paths.state);
    let id = args.id.as_str();
    let Some(p) = state.tasks.proposal(id) else {
        return out.error(Exit::NotFound, &format!("no task proposal `{id}`"));
    };
    let (Some(draft), Some(root)) = (&p.task, &p.root) else {
        return out.error_with_hint(
            Exit::Precondition,
            &format!("`{id}` was only noticed in the hook ledger; no task is drafted yet"),
            Some("run `jouet task propose` in that repository to draft it from its session transcripts"),
        );
    };
    if p.status == Status::Dismissed {
        return out.error(
            Exit::Precondition,
            &format!("`{id}` was dismissed; `jouet task propose` lists the pending proposals"),
        );
    }
    let suggested = draft.suggest.file;
    let file = match args.file.as_deref() {
        None => {
            return out.error_with_hint(
                Exit::Precondition,
                &format!(
                    "choose the file for the task `{}`: ask the user whether it goes in mise.local.toml (just theirs, not committed) or mise.toml (shared with everyone who works on the repository, committed). Suggest {}: {}",
                    draft.name,
                    suggested.config(),
                    draft.suggest.reasons.join("; ")
                ),
                Some(&format!(
                    "then `jouet task apply {id} --file {}`, or `--file {}`",
                    suggested.name(),
                    suggested.other().name()
                )),
            );
        }
        Some(f) => match File::parse(f) {
            Some(file) => file,
            None => {
                return out.error(
                    Exit::Usage,
                    &format!("--file must be local or shared, not `{f}`"),
                );
            }
        },
    };
    let name = args.name.clone().unwrap_or_else(|| draft.name.clone());
    if name.is_empty() || name.starts_with('-') || name.contains(char::is_whitespace) {
        return out.error(
            Exit::Usage,
            &format!("`{name}` is not a task name: no spaces, and no leading -"),
        );
    }
    let root = Path::new(root);
    if !root.is_dir() {
        return out.error(
            Exit::NotFound,
            &format!(
                "the repository `{id}` was drafted in, {}, is gone",
                root.display()
            ),
        );
    }
    let version = match mise::check(root) {
        Ok(version) => version,
        Err(Problem::Failed(message)) => return out.error(Exit::Failed, &message),
        Err(problem) => {
            return out.error_with_hint(
                Exit::Precondition,
                &problem.message(),
                Some("`mise self-update`, or `mise up mise` where mise manages itself"),
            );
        }
    };
    let existing = match mise::tasks(root) {
        Ok(tasks) => tasks,
        Err(problem) => return out.error(Exit::Failed, &problem.message()),
    };
    let root_text = root.to_string_lossy().into_owned();
    let ours = state
        .tasks
        .applied
        .iter()
        .any(|a| a.root == root_text && a.name == name);
    // A task of jouet's changes in place; anyone else's is left alone.
    let verb = if existing.iter().any(|t| t.name == name) {
        if !ours {
            return out.error_with_hint(
                Exit::Precondition,
                &format!("the project already has a task named `{name}`"),
                Some(&format!(
                    "choose another name: `jouet task apply {id} --file {} --name NAME`",
                    file.name()
                )),
            );
        }
        "set"
    } else {
        "add"
    };
    let mut command: Vec<String> = vec!["tasks".into(), verb.into(), "--json".into()];
    if args.dry_run {
        command.push("--dry-run".into());
    }
    match file {
        File::Local => command.extend(["--env".into(), "local".into()]),
        File::Shared => command.extend([
            "--path".into(),
            root.join(shared_config(root).unwrap_or("mise.toml"))
                .to_string_lossy()
                .into_owned(),
        ]),
    }
    command.extend(["--description".into(), draft.description.clone()]);
    for d in &draft.depends {
        command.extend(["--depends".into(), d.clone()]);
    }
    for s in &draft.sources {
        command.extend(["--sources".into(), s.clone()]);
    }
    command.push(name.clone());
    command.push("--".into());
    command.extend(mise::run_words(&draft.run));
    let reported = match mise::json(root, &command) {
        Ok(value) => value,
        Err(problem) => return out.error(Exit::Failed, &problem.message()),
    };
    let path = reported
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let file_name = Path::new(&path).file_name().map_or_else(
        || file.config().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let warning = (file == File::Local && crate::git::ignored(root, &file_name) == Some(false))
        .then(|| format!("{file_name} is not ignored by git here; add it to .gitignore or .git/info/exclude so the task stays yours"));
    if !args.dry_run {
        let run = reported
            .pointer("/table/run")
            .and_then(Value::as_str)
            .unwrap_or(&draft.run)
            .to_string();
        let applied = Applied {
            name: name.clone(),
            root: root_text.clone(),
            path: path.clone(),
            file,
            run,
            proposal: id.to_string(),
            at: wind::now(),
            uses: 0,
            last_used: None,
        };
        let saved = wind::update(&paths.state, |state| {
            state
                .tasks
                .applied
                .retain(|a| !(a.root == applied.root && a.name == applied.name));
            state.tasks.applied.push(applied);
            state.tasks.set_status(id, Status::Adopted);
        });
        if let Err(e) = saved {
            return out.error(Exit::Failed, &format!("writing the winding state: {e}"));
        }
    }
    let report = ApplyReport {
        proposal: id,
        task: &name,
        file,
        mise_version: version,
        command: std::iter::once("mise".to_string())
            .chain(command.iter().cloned())
            .collect(),
        mise: reported,
        warning,
    };
    out.result(&report, || {
        let action = report
            .mise
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("written");
        let would = if args.dry_run { " (dry run: nothing written)" } else { "" };
        let mut text = format!("task {name} {action} in {path}{would}; run it with `mise run {name}`\n");
        if let Some(root) = report.mise.get("trust_root").and_then(Value::as_str) {
            let _ = writeln!(
                text,
                "mise now trusts {root} so that it loads {file_name}; `mise trust --untrust {root}` undoes it"
            );
        }
        if let Some(w) = &report.warning {
            let _ = writeln!(text, "warning: {w}");
        }
        text
    });
    Exit::Ok
}
