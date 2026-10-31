//! `jouet hook HARNESS EVENT`: answers the hooks `jouet setup` installs, by
//! running jouet's rules.
//!
//! The harness sends one JSON document on stdin. Every path fails open: a
//! payload jouet cannot read, a broken config file, a rule that throws or
//! runs out of budget, or a panic exits 0 with no decision, so an error in
//! jouet never blocks the agent. Rule errors are reported on stderr. Exit
//! code 2, which both harnesses read as "block", is never used.

use crate::HookArgs;
use crate::cmd;
use crate::fnox;
use crate::guide;
use crate::ledger::{self, Entry};
use crate::reads;
use crate::rules;
use crate::settings::Settings;
use crate::setup::Harness;
use crate::verify::{self, State};
use crate::wind;
use jouet_rules::{Call, Engine, Event, Outcome, Record};
use jouet_search::memory::Memory;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a hook may spend hashing the work tree a check ran against.
const TREE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(args: &HookArgs) -> jouet_output::exit::Exit {
    let started = std::time::Instant::now();
    let result = std::panic::catch_unwind(|| {
        let harness = args.harness.as_deref().and_then(Harness::parse)?;
        let event = args.event.as_deref()?;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok()?;
        let payload: Value = serde_json::from_str(&input).ok()?;
        if event == "session-start" {
            verify::prune_sessions();
        }
        let settings = Settings::load();
        let hk: Vec<String> = args
            .hk
            .as_deref()
            .map(|c| c.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let output = handle(harness, event, &payload, &settings, &hk);
        record_slow(harness, event, &payload, &settings, started.elapsed());
        output
    });
    match result {
        Ok(Some(output)) => println!("{output}"),
        // Codex reads a Stop hook's stdout as JSON, so it gets an empty
        // object rather than nothing.
        _ if args.event.as_deref() == Some("stop") && args.harness.as_deref() == Some("codex") => {
            println!("{{}}");
        }
        _ => {}
    }
    jouet_output::exit::Exit::Ok
}

/// Records a hook that ran over its budget, for `jouet share --idea`:
/// every tool call waits for it.
fn record_slow(
    harness: Harness,
    event: &str,
    payload: &Value,
    settings: &Settings,
    took: Duration,
) {
    let hooks = &settings.hooks;
    let budget = crate::share::signals::hook_budget(event, hooks.budget_ms);
    let ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
    if !hooks.enabled || !hooks.ledger || ms <= budget {
        return;
    }
    let Some(dir) = ledger::dir() else { return };
    let session = payload.get("session_id").and_then(Value::as_str);
    let mut entry = Entry::new(harness.name(), session, "slow-hook");
    entry.tool = Some(event.to_string());
    entry.duration_ms = Some(ms);
    entry.budget_ms = Some(budget);
    ledger::append(&dir, &entry);
}

/// The hook's stdout for one event, or `None` to print nothing. `hk_stop`
/// is the hk stop hook a merged `stop` runs too.
fn handle(
    harness: Harness,
    event: &str,
    payload: &Value,
    settings: &Settings,
    hk_stop: &[String],
) -> Option<Value> {
    let hooks = &settings.hooks;
    if !hooks.enabled {
        return None;
    }
    let log = Log {
        dir: if hooks.ledger { ledger::dir() } else { None },
        harness: harness.name(),
        session: payload.get("session_id").and_then(Value::as_str),
        repo: payload
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|cwd| ledger::repo_key(cwd.as_ref())),
    };
    match event {
        "session-start" => {
            log.append(log.entry("session-start"));
            let engine = load(payload).filter(|_| hooks.prime)?;
            let primer = session_start(&engine, hooks.steer, hooks.verify);
            let mut notes = wind_notes(settings, log.session, log.repo.as_deref());
            notes.extend(fnox_note(
                payload.get("cwd").and_then(Value::as_str).map(Path::new),
                fnox::under_proxy(),
            ));
            Some(with_notes(primer, &notes))
        }
        "pre-tool-use" if hooks.steer => {
            let engine = load(payload)?;
            let mut call = Call::from_hook(payload, harness.name());
            call.seen = seen(&call, log.session);
            call.read_lines = reads::of(&call).and_then(|r| reads::line_count(&r));
            call.fnox = call
                .cwd
                .as_deref()
                .and_then(|cwd| fnox::context(cwd.as_ref()));
            let (output, outcome) = pre_tool_use(&engine, &call);
            if let Some(outcome) = &outcome {
                log.pre_tool_use(&call, outcome);
            }
            output
        }
        "post-tool-use" | "post-tool-use-failure" => {
            let call = Call::from_hook(payload, harness.name());
            if call.exit_code == Some(0) {
                remember(&call, log.session);
            }
            let (records, tree) = if hooks.verify {
                record(payload, &call)
            } else {
                (Vec::new(), None)
            };
            log.post_tool_use(payload, &call, &records, tree);
            if hooks.graph {
                super::graph::reindex_after_tool(payload, &call);
            }
            None
        }
        "post-edit" => {
            if !settings.edit.fix_after {
                return None;
            }
            let (fixed, root) = fix_after_edit(payload, settings)?;
            crate::ledger::record_fix(
                &root,
                log.session,
                harness.name(),
                payload.get("tool_name").and_then(Value::as_str),
                &fixed,
            );
            let line = fixed.summary()?;
            Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": format!("jouet: {line}"),
                }
            }))
        }
        "stop" => {
            let output = if hooks.verify || !hk_stop.is_empty() {
                stop(harness, payload, hooks.verify, hk_stop)
            } else {
                None
            };
            let mut entry = log.entry("stop");
            if output.as_ref().is_some_and(|o| o.get("decision").is_some()) {
                entry.decisions.push(ledger::Decision {
                    rule: "verify".into(),
                    action: "block".into(),
                    scope: None,
                });
            }
            log.append(entry);
            if settings.wind.enabled
                && let Some(paths) = wind::Paths::locate()
            {
                wind::run(&paths, &settings.wind, crate::wind::now(), false);
                if settings.wind.review {
                    review_on_stop(&paths, settings, harness, payload, log.repo.clone());
                }
            }
            output
        }
        _ => None,
    }
}

/// Whether the session was already shown everything the call reads, for
/// the matchers' `seen`; `None` when the call reads nothing jouet can
/// account for, or there is no session.
fn seen(call: &Call, session: Option<&str>) -> Option<bool> {
    let reads = reads::of(call)?;
    let memory = Memory::open(&cmd::search::seen_dir()?, session?);
    Some(reads::seen(&memory, &reads))
}

/// Adds what a successful call read to the session's memory.
fn remember(call: &Call, session: Option<&str>) {
    let (Some(reads), Some(dir), Some(session)) =
        (reads::of(call), cmd::search::seen_dir(), session)
    else {
        return;
    };
    let mut memory = Memory::open(&dir, session);
    reads::record(&mut memory, &reads);
    memory.save();
}

/// Winds, then picks the notes to show a starting session: at most
/// [`wind::MAX_PER_START`], one line each. The ledger's notes come first,
/// then sessions worth a review and what reviews proposed.
fn wind_notes(settings: &Settings, session: Option<&str>, repo: Option<&str>) -> Vec<String> {
    if !settings.wind.enabled {
        return Vec::new();
    }
    let Some(paths) = wind::Paths::locate() else {
        return Vec::new();
    };
    let now = crate::wind::now();
    wind::run(&paths, &settings.wind, now, false);
    let session = session.unwrap_or_default();
    let mut lines: Vec<String> = wind::notes_for_session(&paths, session)
        .iter()
        .map(|note| wind::note_line(note, session))
        .collect();
    let room = wind::MAX_PER_START.saturating_sub(lines.len());
    lines.extend(wind::review::lines_for_session(
        &paths, session, repo, room, now,
    ));
    if settings.share.contribute
        && lines.len() < wind::MAX_PER_START
        && let Some(gaps) = crate::share::gaps_path()
    {
        let _ = crate::share::refresh_signals(&gaps, &paths);
        lines.extend(crate::share::line_for_session(&gaps, session));
    }
    lines
}

/// At a stop, digests the session (at most every ten minutes) and queues
/// it for review when it crosses a threshold. Reading the transcript is
/// the cost, so only with `[wind] review` on.
fn review_on_stop(
    paths: &wind::Paths,
    settings: &Settings,
    harness: Harness,
    payload: &Value,
    repo: Option<String>,
) {
    let Some(session) = payload.get("session_id").and_then(Value::as_str) else {
        return;
    };
    let Some(transcript) = payload.get("transcript_path").and_then(Value::as_str) else {
        return;
    };
    let now = crate::wind::now();
    if !wind::review::due(paths, session, now) {
        return;
    }
    let redactor = wind::session::Redactor::new(settings.wind.gitleaks.as_deref());
    let Some(digest) = wind::session::digest_path(harness.name(), transcript.as_ref(), &redactor)
    else {
        return;
    };
    crate::share::record_digest(&digest);
    let signals = wind::review::Signals::of(&digest);
    wind::review::consider(
        paths,
        &settings.wind,
        harness.name(),
        session,
        repo,
        signals,
        now,
    );
}

/// The primer with winding notes after it.
fn with_notes(mut primer: Value, notes: &[String]) -> Value {
    if let Some(Value::String(context)) =
        primer.pointer_mut("/hookSpecificOutput/additionalContext")
    {
        for note in notes {
            context.push_str(note);
            context.push('\n');
        }
    }
    primer
}

/// Where this hook's ledger entries go, and whose they are.
struct Log<'a> {
    dir: Option<PathBuf>,
    harness: &'static str,
    session: Option<&'a str>,
    repo: Option<String>,
}

impl Log<'_> {
    fn entry(&self, event: &str) -> Entry {
        Entry {
            repo: self.repo.clone(),
            ..Entry::new(self.harness, self.session, event)
        }
    }

    fn append(&self, entry: Entry) {
        if let Some(dir) = &self.dir {
            ledger::append(dir, &entry);
        }
    }

    /// The call's shape and decisions, after any overrides it is: a retry
    /// of a command of the same shape that a rule denied or warned about.
    fn pre_tool_use(&self, call: &Call, outcome: &Outcome) {
        let (Some(dir), Some(command)) = (&self.dir, &call.command) else {
            return;
        };
        let shape = ledger::shape(command);
        if outcome.deny.is_none()
            && let Some(session) = self.session
        {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            for rule in ledger::overrides(&ledger::recent(dir, session), &shape, now) {
                let mut entry = self.entry("override");
                entry.tool = Some(call.tool.clone());
                entry.shape = Some(shape.clone());
                entry.decisions.push(ledger::Decision {
                    rule,
                    action: "override".into(),
                    scope: None,
                });
                ledger::append(dir, &entry);
            }
        }
        let mut entry = self.entry("pre-tool-use");
        entry.tool = Some(call.tool.clone());
        entry.shape = Some(shape);
        entry.decisions = outcome
            .deny
            .iter()
            .chain(&outcome.warnings)
            .map(|d| ledger::Decision {
                rule: d.rule.clone(),
                action: if d.action == jouet_rules::Action::Deny {
                    "deny".into()
                } else {
                    "warn".into()
                },
                scope: None,
            })
            .collect();
        ledger::append(dir, &entry);
    }

    fn post_tool_use(
        &self,
        payload: &Value,
        call: &Call,
        records: &[Record],
        tree: Option<String>,
    ) {
        let mut entry = self.entry("post-tool-use");
        entry.tool = Some(call.tool.clone());
        entry.shape = call.command.as_deref().map(ledger::shape);
        entry.exit_code = call.exit_code;
        entry.duration_ms = payload.get("duration_ms").and_then(Value::as_u64);
        entry.tree = tree;
        entry.decisions = records
            .iter()
            .map(|r| ledger::Decision {
                rule: r.rule.clone(),
                action: format!("record:{}", r.kind.name()),
                scope: Some(if r.full { "full" } else { "partial" }.into()),
            })
            .collect();
        self.append(entry);
    }
}

/// The rules for the session's directory, or for this process's.
fn load(payload: &Value) -> Option<Engine> {
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    let loaded = rules::load(&cwd);
    for error in &loaded.errors {
        eprintln!("jouet: {error}");
    }
    Some(loaded.engine)
}

fn evaluate(engine: &Engine, call: &Call) -> Outcome {
    let outcome = engine.evaluate(call);
    for error in &outcome.errors {
        eprintln!("jouet: {error}");
    }
    crate::share::record_gaps(engine);
    outcome
}

fn session_start(engine: &Engine, steer: bool, verify: bool) -> Value {
    let rules: Vec<&str> = if steer {
        engine
            .rules()
            .iter()
            .filter(|r| r.state == jouet_rules::State::Enabled && r.event == Event::PreToolUse)
            // A rule about project files not here never fires here.
            .filter(|r| engine.in_use_here(r))
            .map(|r| r.name.as_str())
            .collect()
    } else {
        Vec::new()
    };
    let mut context = guide::summary(&rules, verify);
    // A deprecated Luau rule file: the agent can convert it.
    for note in engine
        .rules()
        .iter()
        .filter_map(jouet_rules::Rule::deprecation)
    {
        context.push_str(&format!("jouet: {note}\n"));
    }
    json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })
}

/// What the session should know about fnox: that it runs under `fnox proxy
/// run`, or that the project keeps its secrets in fnox.
fn fnox_note(cwd: Option<&Path>, proxied: bool) -> Option<String> {
    if proxied {
        return Some(
            "jouet: this session runs under `fnox proxy run`: its secret variables hold placeholders, and fnox puts the real values only into the HTTPS requests its rules allow. A placeholder is not a broken credential; don't try to read the real value."
                .into(),
        );
    }
    cwd.and_then(fnox::Project::find).map(|_| {
        "jouet: this project keeps its secrets in fnox. Run what needs them with `fnox exec -- CMD`, and keep their values out of the transcript: no `env`, `printenv`, `fnox get` or reading `.env` files."
            .to_string()
    })
}

/// Denies a call a rule denies; otherwise passes on any warnings. Also
/// returns what the rules decided, for the ledger. `setup` sends only shell
/// commands here, but a rule matching a file tool's `paths` runs for any
/// tool call the harness sends.
fn pre_tool_use(engine: &Engine, call: &Call) -> (Option<Value>, Option<Outcome>) {
    if call.tool == "Bash" && call.command.is_none() {
        return (None, None);
    }
    let call = Call {
        event: Some(Event::PreToolUse),
        ..call.clone()
    };
    let outcome = evaluate(engine, &call);
    (respond(&outcome), Some(outcome))
}

fn respond(outcome: &Outcome) -> Option<Value> {
    let tag = |d: &jouet_rules::Decision| {
        format!(
            "{} [jouet rule: {}]",
            d.message.as_deref().unwrap_or("jouet denied this command"),
            d.rule
        )
    };
    if let Some(deny) = &outcome.deny {
        return Some(json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": tag(deny),
            }
        }));
    }
    if outcome.warnings.is_empty() {
        return None;
    }
    let context: Vec<String> = outcome.warnings.iter().map(tag).collect();
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": context.join("\n"),
        }
    }))
}

/// Runs the post-tool-use rules on a finished Bash command and keeps what
/// they record in the session's state file, with the tree the command ran
/// against, which is also returned.
fn record(payload: &Value, call: &Call) -> (Vec<Record>, Option<String>) {
    // A background command has not finished when this hook runs.
    let background = payload
        .get("tool_input")
        .and_then(|i| i.get("run_in_background"))
        .and_then(Value::as_bool)
        == Some(true);
    if call.tool != "Bash" || call.exit_code.is_none() || background {
        return (Vec::new(), None);
    }
    let Some(path) = session_state_path(payload) else {
        return (Vec::new(), None);
    };
    let Some(engine) = load(payload) else {
        return (Vec::new(), None);
    };
    let call = Call {
        event: Some(Event::PostToolUse),
        ..call.clone()
    };
    let records = evaluate(&engine, &call).records;
    if records.is_empty() {
        return (records, None);
    }
    // The command has finished, so the tree now is the one it ran against,
    // unless the command itself changed tracked files.
    let tree = work_tree(payload);
    let _ = verify::update(&path, |state| {
        state.record(records.clone(), tree.as_deref());
    });
    (records, tree)
}

/// The session's work tree, hashed; None outside a repository.
fn work_tree(payload: &Value) -> Option<String> {
    let cwd = payload.get("cwd").and_then(Value::as_str).map(Path::new);
    jouet_rules::tree::work_tree(cwd, TREE_TIMEOUT).ok()
}

fn session_state_path(payload: &Value) -> Option<PathBuf> {
    verify::state_path(payload.get("session_id")?.as_str()?)
}

/// The stop gate: jouet's verify and, when setup merged it, hk's own stop
/// hook, as one decision. hk is not run when the session's last hk check
/// passed at the tree as it is now.
fn stop(harness: Harness, payload: &Value, verify: bool, hk_stop: &[String]) -> Option<Value> {
    // Jobs left running are a note, never a reason to block.
    let note = if verify { jobs_note(payload) } else { None };
    let as_note = |note: Option<String>| note.map(|note| json!({ "systemMessage": note }));
    // Block at most once in a row: after one nudge, the agent's answer
    // stands, so a check that cannot pass never traps it.
    if payload.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return as_note(note);
    }
    let state = match harness {
        Harness::Claude => session_state_path(payload).map(|p| State::load(&p)),
        Harness::Codex => payload
            .get("transcript_path")
            .and_then(Value::as_str)
            .map(|t| verify::codex_commands(t.as_ref()))
            .filter(|commands| !commands.is_empty())
            .and_then(|commands| Some(codex_state(&load(payload)?, commands))),
    }
    .unwrap_or_default();
    let tree = std::cell::OnceCell::new();
    let now = || tree.get_or_init(|| work_tree(payload)).clone();
    let root = payload
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(|cwd| Path::new(cwd).ancestors().find(|d| d.join(".git").exists()))
        .map(Path::to_path_buf);
    let hk_repo = root.as_deref().and_then(crate::diag::hk_config).is_some();
    let jouet = if verify {
        state.block_reason(now, hk_repo)
    } else {
        None
    };
    let hk = if hk_stop.is_empty() || state.hk_passed_at(now) {
        None
    } else {
        run_hk_stop(hk_stop, payload)
    };
    let reason = match (jouet, hk) {
        (None, None) => return as_note(note),
        (Some(one), None) => one,
        (None, Some(hk)) => format!("{hk} [hk]"),
        (Some(jouet), Some(hk)) => format!("{jouet}\n\nhk check: {hk} [hk]"),
    };
    let reason = match &note {
        Some(note) => format!("{reason}\n{note}"),
        None => reason,
    };
    Some(json!({ "decision": "block", "reason": reason }))
}

/// What the stop says about the `jouet job`s this session started in this
/// work tree that are still running, if there are any.
fn jobs_note(payload: &Value) -> Option<String> {
    let session = payload.get("session_id").and_then(Value::as_str)?;
    let cwd = payload.get("cwd").and_then(Value::as_str)?;
    let store = jouet_wait::job::Store::here(Path::new(cwd))?;
    let running: Vec<String> = store
        .list()
        .into_iter()
        .filter(|j| j.session.as_deref() == Some(session))
        .filter(|j| store.state(j) == jouet_wait::job::State::Running)
        .map(|j| j.id)
        .collect();
    jobs_left(&running)
}

/// The note for jobs left running.
fn jobs_left(running: &[String]) -> Option<String> {
    let (what, it, it_is) = match running {
        [] => return None,
        [_] => ("A job this session started is", "it", "it"),
        _ => ("Jobs this session started are", "them", "they"),
    };
    let names: Vec<String> = running.iter().map(|id| format!("`{id}`")).collect();
    Some(format!(
        "{what} still running: {}. Leave {it} if {it_is} should stay up; `jouet job stop {}` stops {it}. [jouet: jobs]",
        names.join(", "),
        running.join(" ")
    ))
}

/// Runs hk's stop hook with this hook's payload and returns the reason it
/// blocks with, if it does. Fails open: an hk that cannot run says nothing.
fn run_hk_stop(argv: &[String], payload: &Value) -> Option<String> {
    let (program, args) = argv.split_first()?;
    let dir = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let mut child = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write as _;
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    let value: Value = serde_json::from_slice(&out.stdout).ok()?;
    (value.get("decision").and_then(Value::as_str) == Some("block"))
        .then(|| {
            value
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .flatten()
}

/// Runs hk's fixers on the file an Edit or Write call changed, within
/// `[edit] fix_timeout_ms`. None outside a repository with an hk config.
fn fix_after_edit(
    payload: &Value,
    settings: &Settings,
) -> Option<(crate::diag::fix::Fixed, PathBuf)> {
    let input = payload.get("tool_input")?;
    let file = input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))
        .and_then(Value::as_str)?;
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let path = cwd.join(file);
    let root = path
        .ancestors()
        .skip(1)
        .find(|d| d.join(".git").exists())?
        .to_path_buf();
    crate::diag::hk_config(&root)?;
    let root = root.canonicalize().unwrap_or(root);
    let path = path.canonicalize().ok()?;
    let rel = crate::diag::relative(&root, &path.to_string_lossy());
    let steps = settings.edit.fix_steps.clone().unwrap_or_default();
    let timeout = Duration::from_millis(settings.edit.fix_timeout_ms);
    Some((crate::diag::fix::run(&root, &[rel], &steps, timeout), root))
}

/// Runs a Codex session's finished commands through the post-tool-use
/// rules, oldest first.
fn codex_state(engine: &Engine, commands: Vec<(String, i32)>) -> State {
    let mut state = State::default();
    for (command, exit_code) in commands {
        let call = Call {
            event: Some(Event::PostToolUse),
            harness: "codex".into(),
            tool: "Bash".into(),
            command: Some(command),
            path: None,
            cwd: None,
            exit_code: Some(exit_code),
            ..Call::default()
        };
        // The transcript does not say what tree each command saw.
        state.record(evaluate(engine, &call).records, None);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use jouet_rules::{Budget, Options, Origin, Source};

    fn engine(extra: Vec<Source>) -> Engine {
        let mut sources = jouet_rules::builtin();
        sources.extend(extra);
        Engine::new(
            sources,
            Options {
                has_command: Box::new(crate::spec::has_command),
                disabled: Vec::new(),
                budget: Budget::default(),
                cache_dir: None,
            },
        )
    }

    fn bash(command: &str) -> Call {
        Call::from_hook(
            &json!({"tool_name": "Bash", "tool_input": {"command": command}}),
            "claude",
        )
    }

    #[test]
    fn denies_with_the_rules_advice_and_name() {
        let out = pre_tool_use(&engine(Vec::new()), &bash("git add -A"))
            .0
            .unwrap();
        let output = &out["hookSpecificOutput"];
        assert_eq!(output["permissionDecision"], "deny");
        let reason = output["permissionDecisionReason"].as_str().unwrap();
        assert!(
            reason.starts_with("Run `git add path/to/file ...` instead."),
            "{reason}"
        );
        assert!(reason.ends_with("[jouet rule: git-add-all]"), "{reason}");
    }

    #[test]
    fn says_nothing_about_other_commands_and_tools() {
        let engine = engine(Vec::new());
        assert_eq!(pre_tool_use(&engine, &bash("git add src/main.rs")).0, None);
        let edit = Call::from_hook(&json!({"tool_name": "Edit"}), "claude");
        assert_eq!(pre_tool_use(&engine, &edit).0, None);
    }

    #[test]
    fn a_rule_can_deny_a_file_tool_by_path() {
        let rule = Source::toml(
            "locks",
            Origin::User("locks.toml".into()),
            "[[rule]]\nname = \"locks\"\nwhy = \"generated\"\nmode = \"deny\"\nmessage = \"Regenerate it.\"\nmatch = { tool = \"Edit\", paths = \"*.lock\" }\nexamples = { fires = [{ tool = \"Edit\", path = \"a.lock\" }], quiet = [{ tool = \"Edit\", path = \"a.rs\" }] }\n",
        );
        let engine = engine(vec![rule]);
        let edit = |path: &str| {
            Call::from_hook(
                &json!({"tool_name": "Edit", "tool_input": {"file_path": path}}),
                "claude",
            )
        };
        let out = pre_tool_use(&engine, &edit("/repo/Cargo.lock")).0.unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(pre_tool_use(&engine, &edit("/repo/src/main.rs")).0, None);
    }

    #[test]
    fn a_warning_becomes_context() {
        let warn = Source::luau(
            "warn-ls",
            Origin::User("warn-ls.luau".into()),
            "--- why: test\nreturn { check = function(c) if c.command == 'ls' then return jouet.warn('try eza') end end }",
        );
        let out = pre_tool_use(&engine(vec![warn]), &bash("ls")).0.unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            "try eza [jouet rule: warn-ls]"
        );
        assert!(
            out["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
        );
    }

    #[test]
    fn codex_commands_replay_through_the_post_tool_use_rules() {
        let engine = engine(Vec::new());
        let failing = codex_state(
            &engine,
            vec![("cargo test --workspace".into(), 101), ("ls".into(), 0)],
        );
        assert!(failing.block_reason(|| None, false).is_some());
        let fixed = codex_state(
            &engine,
            vec![
                ("cargo test --workspace".into(), 101),
                ("cargo test --workspace".into(), 0),
            ],
        );
        assert_eq!(fixed.block_reason(|| None, false), None);
        // A partial pass leaves the full failure standing.
        let partial = codex_state(
            &engine,
            vec![
                ("cargo test --workspace".into(), 101),
                ("cargo test -p jouet --lib".into(), 0),
            ],
        );
        let why = partial.block_reason(|| None, false).unwrap();
        assert!(why.contains("only a full test run clears it"), "{why}");
    }

    #[test]
    fn winding_notes_follow_the_primer() {
        let primer = session_start(&engine(Vec::new()), true, true);
        let before = primer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        let out = with_notes(primer, &["note one".into(), "note two".into()]);
        let context = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(context, format!("{before}note one\nnote two\n"));
    }

    #[test]
    fn the_jobs_note_names_each_job() {
        assert_eq!(jobs_left(&[]), None);
        let one = jobs_left(&["dev".into()]).unwrap();
        assert!(
            one.starts_with(
                "A job this session started is still running: `dev`. Leave it if it should stay up;"
            ),
            "{one}"
        );
        let two = jobs_left(&["dev".into(), "api".into()]).unwrap();
        assert!(two.contains("`jouet job stop dev api` stops them"), "{two}");
    }

    #[test]
    fn the_primer_names_the_active_rules() {
        let out = session_start(&engine(Vec::new()), true, true);
        let context = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("git-stash"), "{context}");
        assert!(!context.contains("checks"), "{context}");
        assert_eq!(
            context.contains("sleep-poll"),
            crate::spec::has_command(&["wait", "ci"])
        );
    }

    #[test]
    fn the_primer_mentions_fnox_where_it_matters() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fnox_note(Some(dir.path()), false), None);
        assert!(
            fnox_note(None, true)
                .unwrap()
                .contains("runs under `fnox proxy run`")
        );
        std::fs::write(dir.path().join("fnox.toml"), "[secrets]\n").unwrap();
        assert!(
            fnox_note(Some(dir.path()), false)
                .unwrap()
                .contains("`fnox exec -- CMD`")
        );
    }
}
