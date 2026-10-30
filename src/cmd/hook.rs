//! `jouet hook HARNESS EVENT`: answers the hooks `jouet setup` installs, by
//! running jouet's rules.
//!
//! The harness sends one JSON document on stdin. Every path fails open: a
//! payload jouet cannot read, a broken config file, a rule that throws or
//! runs out of budget, or a panic exits 0 with no decision, so an error in
//! jouet never blocks the agent. Rule errors are reported on stderr. Exit
//! code 2, which both harnesses read as "block", is never used.

use crate::HookArgs;
use crate::guide;
use crate::ledger::{self, Entry};
use crate::rules;
use crate::settings::Settings;
use crate::setup::Harness;
use crate::verify::{self, State};
use crate::wind;
use jouet_rules::{Call, Engine, Event, Outcome, Record};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a hook may spend hashing the work tree a check ran against.
const TREE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(args: &HookArgs) -> jouet_output::exit::Exit {
    let result = std::panic::catch_unwind(|| {
        let harness = args.harness.as_deref().and_then(Harness::parse)?;
        let event = args.event.as_deref()?;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok()?;
        let payload: Value = serde_json::from_str(&input).ok()?;
        if event == "session-start" {
            verify::prune_sessions();
        }
        handle(harness, event, &payload, &Settings::load())
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

/// The hook's stdout for one event, or `None` to print nothing.
fn handle(harness: Harness, event: &str, payload: &Value, settings: &Settings) -> Option<Value> {
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
            Some(with_notes(primer, &wind_notes(settings, log.session)))
        }
        "pre-tool-use" if hooks.steer => {
            let engine = load(payload)?;
            let call = Call::from_hook(payload, harness.name());
            let (output, outcome) = pre_tool_use(&engine, &call);
            if let Some(outcome) = &outcome {
                log.pre_tool_use(&call, outcome);
            }
            output
        }
        "post-tool-use" | "post-tool-use-failure" => {
            let call = Call::from_hook(payload, harness.name());
            let (records, tree) = if hooks.verify {
                record(payload, &call)
            } else {
                (Vec::new(), None)
            };
            log.post_tool_use(payload, &call, &records, tree);
            None
        }
        "stop" => {
            let output = if hooks.verify {
                stop(harness, payload)
            } else {
                None
            };
            let mut entry = log.entry("stop");
            if output.is_some() {
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
            }
            output
        }
        _ => None,
    }
}

/// Winds, then picks the notes to show a starting session: at most
/// [`wind::MAX_PER_START`], one line each.
fn wind_notes(settings: &Settings, session: Option<&str>) -> Vec<String> {
    if !settings.wind.enabled {
        return Vec::new();
    }
    let Some(paths) = wind::Paths::locate() else {
        return Vec::new();
    };
    wind::run(&paths, &settings.wind, crate::wind::now(), false);
    let session = session.unwrap_or_default();
    wind::notes_for_session(&paths, session)
        .iter()
        .map(|note| wind::note_line(note, session))
        .collect()
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

fn stop(harness: Harness, payload: &Value) -> Option<Value> {
    // Block at most once in a row: after one nudge, the agent's answer
    // stands, so a check that cannot pass never traps it.
    if payload.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let state = match harness {
        Harness::Claude => State::load(&session_state_path(payload)?),
        Harness::Codex => {
            let transcript = payload.get("transcript_path")?.as_str()?;
            let commands = verify::codex_commands(transcript.as_ref());
            if commands.is_empty() {
                return None;
            }
            let engine = load(payload)?;
            codex_state(&engine, commands)
        }
    };
    let reason = state.block_reason(|| work_tree(payload))?;
    Some(json!({ "decision": "block", "reason": reason }))
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
        assert!(failing.block_reason(|| None).is_some());
        let fixed = codex_state(
            &engine,
            vec![
                ("cargo test --workspace".into(), 101),
                ("cargo test --workspace".into(), 0),
            ],
        );
        assert_eq!(fixed.block_reason(|| None), None);
        // A partial pass leaves the full failure standing.
        let partial = codex_state(
            &engine,
            vec![
                ("cargo test --workspace".into(), 101),
                ("cargo test -p jouet --lib".into(), 0),
            ],
        );
        let why = partial.block_reason(|| None).unwrap();
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
}
