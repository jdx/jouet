//! `jouet hook HARNESS EVENT`: answers the hooks `jouet setup` installs, by
//! running jouet's rules.
//!
//! The harness sends one JSON document on stdin, which [`harness`] reads as
//! Claude Code's and whose answer it writes in the harness's own fields.
//! Every path fails open: a payload jouet cannot read, a broken config file,
//! a rule that throws or runs out of budget, or a panic exits 0 with no
//! decision, so an error in jouet never blocks the agent. Rule errors are
//! reported on stderr. Exit code 2, which harnesses read as "block", is
//! never used.

use crate::agents;
use crate::cli::HookArgs;
use crate::cmd;
use crate::fnox;
use crate::guide;
use crate::harness;
use crate::ledger::{self, Entry};
use crate::reads;
use crate::rules;
use crate::settings::Settings;
use crate::setup::Harness;
use crate::sources::Status;
use crate::testlog;
use crate::verify::{self, State};
use crate::wind;
use jouet_rules::{Call, Engine, Event, Outcome, Record};
use jouet_search::memory::Memory;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a hook may spend hashing the work tree a check ran against.
pub(crate) const TREE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(args: &HookArgs) -> jouet_output::exit::Exit {
    let started = std::time::Instant::now();
    let harness = args.harness.as_deref().and_then(Harness::parse);
    let result = std::panic::catch_unwind(|| {
        let harness = harness?;
        let event = args.event.as_deref()?;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok()?;
        let payload: Value = serde_json::from_str(&input).ok()?;
        if !first_run(harness, event, &payload) {
            return None;
        }
        let payload = harness::normalize(harness, event, payload);
        if harness == Harness::Claude && cursor_runs_its_own(&payload) {
            return None;
        }
        if event == "session-start" {
            verify::prune_sessions();
            agents::prune_calls();
        }
        let settings = Settings::load();
        let hk: Vec<String> = args
            .hk
            .as_deref()
            .map(|c| c.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let output = handle(harness, event, &payload, &settings, &hk);
        record_slow(harness, event, &payload, &settings, started.elapsed());
        harness::render(harness, event, output)
    });
    match (result, harness) {
        (Ok(Some(output)), _) => println!("{output}"),
        // These read the hook's stdout as JSON, so they get an empty object
        // rather than nothing: Codex at a stop, the others always.
        (_, Some(h)) if h.wants_json() => println!("{{}}"),
        (_, Some(Harness::Codex)) if args.event.as_deref() == Some("stop") => println!("{{}}"),
        _ => {}
    }
    jouet_output::exit::Exit::Ok
}

/// Whether this Claude Code hook is Cursor running it, which it does for
/// the hooks in Claude Code's settings files, while jouet's own Cursor hooks
/// are installed too: they answer instead, so a call gets one verdict.
/// Cursor sets `CURSOR_VERSION` for its hooks; Claude Code sets `CLAUDECODE`.
fn cursor_runs_its_own(payload: &Value) -> bool {
    if std::env::var_os("CURSOR_VERSION").is_none() || std::env::var_os("CLAUDECODE").is_some() {
        return false;
    }
    let has = |path: PathBuf| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|doc| crate::setup::has_jouet(&doc))
    };
    let global = std::env::home_dir().is_some_and(|home| {
        has(Harness::Cursor.config_path(crate::setup::Scope::Global, &home, &home))
    });
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CURSOR_PROJECT_DIR").map(PathBuf::from));
    global
        || cwd.is_some_and(|cwd| {
            cwd.ancestors()
                .find(|d| d.join(".git").exists())
                .is_some_and(|root| has(root.join(".cursor").join("hooks.json")))
        })
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
/// How long a hook's payload is remembered, to answer it once.
const RUN_ONCE: Duration = Duration::from_secs(5);

/// Whether this is the first time this hook got this payload. jouet's hooks
/// installed twice, at user scope and in a project's committed settings with
/// an older guard, are two commands to the harness, so it runs both on the
/// same payload: the agent then reads the primer, and every warning and
/// denial, twice, and the ledger counts everything twice. Only the first
/// answers. A payload counts only with what makes it one call: a tool call's
/// `tool_use_id`, a subagent's `agent_id`, a prompt's `prompt_id`, or a session start's transcript, which cannot start twice
/// within [`RUN_ONCE`] otherwise. A stop has no such id, and two stops of a
/// short turn can look the same, so each is answered.
fn first_run(harness: Harness, event: &str, payload: &Value) -> bool {
    use std::hash::{Hash, Hasher};
    let key = match event {
        "session-start" => "transcript_path",
        "subagent-start" => "agent_id",
        "user-prompt-submit" => "prompt_id",
        _ => "tool_use_id",
    };
    let id = payload.get(key).and_then(Value::as_str).is_some();
    let Some(dir) = jouet_output::paths::state_dir().map(|d| d.join("hook-runs")) else {
        return true;
    };
    if !id || std::fs::create_dir_all(&dir).is_err() {
        return true;
    }
    let now = SystemTime::now();
    let stale = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|at| now.duration_since(at).unwrap_or_default() > RUN_ONCE)
    };
    if event == "session-start" {
        for marker in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if stale(&marker.path()) {
                let _ = std::fs::remove_file(marker.path());
            }
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (harness.name(), event, payload.to_string()).hash(&mut hasher);
    let path = dir.join(format!("{:016x}", hasher.finish()));
    if stale(&path) {
        let _ = std::fs::remove_file(&path);
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Err(e) => e.kind() != std::io::ErrorKind::AlreadyExists,
        Ok(_) => true,
    }
}

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
        agent: agents::of_payload(payload),
        repo: payload
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|cwd| ledger::repo_key(cwd.as_ref())),
    };
    // The session's experiment arm: which toggles keep their old behaviour.
    let arm = settings.arm(log.session);
    let gate_blocks = hooks.gate_blocks() || arm.gate_blocks;
    match event {
        // Claude Code shows a subagent nothing of its session's context, so
        // it gets the primer too, in its own start event. The session was
        // counted when it started, and its winding notes are for the lead.
        "session-start" | "subagent-start" => {
            let subagent = event == "subagent-start";
            if !subagent {
                let mut entry = log.entry("session-start");
                if settings.experiment.control_share > 0 && log.session.is_some() {
                    entry.arm = arm.labels();
                }
                log.append(entry);
            }
            let loaded = load_all(payload).filter(|_| hooks.prime)?;
            let cwd = payload.get("cwd").and_then(Value::as_str).map(Path::new);
            let mut primer = session_start(
                &loaded.engine,
                hooks.steer,
                hooks.verify && gate_blocks,
                harness,
                cwd,
            );
            // The slow checks here, from the timings kept in git notes.
            let mut notes: Vec<String> = cwd
                .and_then(|cwd| crate::cost::primer_line(&crate::cost::Costs::load(cwd)))
                .into_iter()
                .collect();
            // The checks the project committed, so the agent need not look.
            notes.extend(cwd.and_then(crate::project::primer_line));
            // And where things are, read when needed.
            notes.extend(cwd.and_then(crate::project::map_line));
            if subagent {
                primer["hookSpecificOutput"]["hookEventName"] = json!("SubagentStart");
            } else {
                notes.extend(wind_notes(settings, log.session, log.repo.as_deref(), cwd));
            }
            notes.extend(untrusted_note(&loaded));
            notes.extend(fnox_note(
                payload.get("cwd").and_then(Value::as_str).map(Path::new),
                fnox::under_proxy(),
            ));
            Some(with_notes(primer, &notes))
        }
        // What started the turn, recorded and passed on unchanged: the
        // desktop app's events reach this hook as prompts, and nothing else
        // tells them apart from what the user typed.
        "user-prompt-submit" => {
            let prompt = payload.get("prompt").and_then(Value::as_str)?;
            let mut entry = log.entry("prompt");
            entry.prompt = Some(crate::prompt::classify(prompt));
            log.append(entry);
            None
        }
        // Without steering no rule runs, but the stop gate still wants the
        // tree a formatting call started from.
        "pre-tool-use" if !hooks.steer && hooks.verify => {
            note_formatting(payload, &Call::from_hook(payload, harness.name()), true);
            if harness == Harness::Claude {
                agents::note_call(payload);
            }
            None
        }
        "pre-tool-use" if hooks.steer => {
            let engine = load(payload)?;
            let mut call = Call::from_hook(payload, harness.name());
            call.seen = seen(&call, log.session, agents::of_payload(payload));
            let read = reads::of(&call);
            call.read_lines = read.as_deref().and_then(reads::line_count);
            call.code_lines = read.as_deref().and_then(reads::code_line_count);
            call.tracked_files = tracked_files(&call);
            call.fnox = call
                .cwd
                .as_deref()
                .and_then(|cwd| fnox::context(cwd.as_ref()));
            let (mut output, outcome) = pre_tool_use(&engine, &call);
            if let Some(outcome) = &outcome {
                log.pre_tool_use(&call, outcome);
            }
            if harness == Harness::Claude && outcome.as_ref().is_some_and(|o| o.deny.is_none()) {
                output = with_long_timeout(&engine, payload, &call, output);
            }
            if hooks.verify && outcome.as_ref().is_some_and(|o| o.deny.is_none()) {
                note_formatting(payload, &call, true);
            }
            if harness == Harness::Claude && outcome.as_ref().is_none_or(|o| o.deny.is_none()) {
                agents::note_call(payload);
            }
            // A warning waits for the call's post-tool-use hook where the
            // harness has nowhere else to put it.
            if harness::defers_warnings(harness)
                && let Some(outcome) = outcome.filter(|o| o.deny.is_none())
                && !outcome.warnings.is_empty()
            {
                let context = output.as_ref().and_then(|o| harness::Answer::of(o).context);
                if let (Some(path), Some(context)) = (pending_path(payload), context) {
                    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
                    let _ = std::fs::write(path, context);
                }
                return None;
            }
            output
        }
        "post-tool-use" | "post-tool-use-failure" => {
            let call = Call::from_hook(payload, harness.name());
            if call.exit_code == Some(0) {
                remember(&call, log.session, agents::of_payload(payload));
            }
            if harness == Harness::Claude {
                agents::end_call(payload);
            }
            let outcome = (hooks.verify || hooks.steer)
                .then(|| load(payload))
                .flatten()
                .map(|engine| post_tool_use(&engine, payload, &call))
                .unwrap_or_default();
            let records = if hooks.verify {
                outcome.records
            } else {
                Vec::new()
            };
            let warnings = if hooks.steer {
                outcome.warnings
            } else {
                Vec::new()
            };
            let ran = record(payload, &call, &records);
            log.post_tool_use(payload, &call, &records, &warnings, ran);
            if hooks.verify {
                note_formatting(payload, &call, false);
            }
            if call.tool == "Bash" {
                keep_cost(payload, &call, &records, hooks.verify);
            }
            // A run of a mise task jouet added, for `jouet unwind`.
            if settings.wind.enabled
                && let (Some(command), Some(cwd)) = (call.command.as_deref(), call.cwd.as_deref())
                && command.contains("mise")
                && let Some(paths) = wind::Paths::locate()
            {
                wind::tasks::record_use(&paths, Path::new(cwd), command, crate::wind::now());
            }
            // A warning the pre-tool-use hook held for this call goes first.
            let held = harness::defers_warnings(harness)
                .then(|| pending_path(payload))
                .flatten()
                .and_then(|path| {
                    let context = std::fs::read_to_string(&path).ok();
                    let _ = std::fs::remove_file(&path);
                    context
                });
            after_tool(event, held, &warnings)
        }
        // Every harness sends its file edits here, Codex its patches too.
        // The graph is re-indexed after the format, so it sees the file as
        // it was left.
        "post-edit" => {
            // An edit after a stop the gate found a reason at is what
            // `jouet stats` judges that reason by.
            if hooks.verify {
                let mut entry = log.entry("edit");
                entry.tool = payload
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                log.append(entry);
            }
            let fixed = settings
                .edit
                .fix_after
                .then(|| fix_after_edit(payload, settings))
                .flatten();
            if hooks.graph {
                let call = Call::from_hook(payload, harness.name());
                super::graph::reindex_after_tool(payload, &call);
            }
            let (fixed, root) = fixed?;
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
            let gate = hooks.verify.then_some(gate_blocks);
            let Stopped {
                output,
                reasons,
                hk_blocked,
            } = if hooks.verify || !hk_stop.is_empty() {
                stop(harness, payload, gate, hk_stop)
            } else {
                Stopped::default()
            };
            let mut entry = log.entry("stop");
            // One decision per reason, each named: a `block` where the gate
            // stopped the agent, a `would-block` where it only observed.
            let action = if gate == Some(true) {
                "block"
            } else {
                "would-block"
            };
            entry
                .decisions
                .extend(reasons.into_iter().map(|reason| ledger::Decision {
                    rule: "verify".into(),
                    action: action.into(),
                    scope: Some(reason),
                    revision: None,
                }));
            if hk_blocked {
                entry.decisions.push(ledger::Decision {
                    rule: "verify".into(),
                    action: "block".into(),
                    scope: Some("hk-stop".into()),
                    revision: None,
                });
            }
            log.append(entry);
            if settings.wind.enabled
                && let Some(paths) = wind::Paths::locate()
            {
                let now = crate::wind::now();
                wind::run(&paths, &settings.wind, now, false);
                // GitHub is asked in the background, at most once a day.
                if settings.wind.memories
                    && wind::memories::Roots::from_env().any()
                    && wind::memories::due(&paths, now)
                {
                    wind::memories::spawn_check();
                }
                if settings.wind.review {
                    review_on_stop(&paths, settings, harness, payload, log.repo.clone());
                }
            }
            output
        }
        _ => None,
    }
}

/// Where a pre-tool-use warning waits for its call's post-tool-use hook:
/// beside the session's state file, so files left when a call never ran go
/// with it after a week. The call is its `tool_use_id`, or its tool and
/// input, which a harness sends both hooks alike.
fn pending_path(payload: &Value) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let call = match payload.get("tool_use_id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => format!(
            "{}\n{}",
            payload.get("tool_name")?,
            payload.get("tool_input").unwrap_or(&Value::Null)
        ),
    };
    let hash: String = Sha256::digest(call.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    let state = session_state_path(payload)?;
    let stem = state.file_stem()?.to_str()?;
    Some(state.with_file_name(format!("{stem}.warning-{hash}.txt")))
}

/// Whether the session was already shown everything the call reads, for
/// the matchers' `seen`; `None` when the call reads nothing jouet can
/// account for, or there is no session.
fn seen(call: &Call, session: Option<&str>, agent: Option<&str>) -> Option<bool> {
    let reads = reads::of(call)?;
    let memory = Memory::open(&cmd::search::seen_dir()?, &agents::key(session?, agent));
    Some(reads::seen(&memory, &reads))
}

/// Adds what a successful call read to the memory of the session's agent
/// that made it.
fn remember(call: &Call, session: Option<&str>, agent: Option<&str>) {
    let (Some(reads), Some(dir), Some(session)) =
        (reads::of(call), cmd::search::seen_dir(), session)
    else {
        return;
    };
    let mut memory = Memory::open(&dir, &agents::key(session, agent));
    reads::record(&mut memory, &reads);
    memory.save();
}

/// Winds, then picks the notes to show a starting session: at most
/// [`wind::MAX_PER_START`], one line each. The ledger's notes come first,
/// then mise tasks proposed for this repository, then stale memories of
/// this project, then sessions worth a review and what reviews proposed.
fn wind_notes(
    settings: &Settings,
    session: Option<&str>,
    repo: Option<&str>,
    cwd: Option<&Path>,
) -> Vec<String> {
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
    lines.extend(wind::tasks::lines_for_session(&paths, session, repo, room));
    if settings.wind.memories {
        let room = wind::MAX_PER_START.saturating_sub(lines.len());
        lines.extend(wind::memories::lines_for_session(
            &paths, session, cwd, room,
        ));
    }
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
    // Only Claude Code's and Codex's transcripts can be read.
    if !matches!(harness, Harness::Claude | Harness::Codex) {
        return;
    }
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

/// How many bytes of output a tool call gave the agent: Claude Code's
/// `tool_response`, a shell's `stdout` and `stderr` or a tool's text. None
/// when the payload carries none.
fn output_bytes(payload: &Value) -> Option<u64> {
    let len = |v: &Value| v.as_str().map(|s| s.len() as u64);
    match payload.get("tool_response")? {
        Value::String(s) => Some(s.len() as u64),
        Value::Object(o) if o.contains_key("stdout") || o.contains_key("stderr") => Some(
            ["stdout", "stderr"]
                .iter()
                .filter_map(|k| o.get(*k).and_then(len))
                .sum(),
        ),
        _ => None,
    }
}

/// Where this hook's ledger entries go, and whose they are.
struct Log<'a> {
    dir: Option<PathBuf>,
    harness: &'static str,
    session: Option<&'a str>,
    agent: Option<&'a str>,
    repo: Option<String>,
}

impl Log<'_> {
    fn entry(&self, event: &str) -> Entry {
        Entry {
            agent: self.agent.map(ledger::redact_id),
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
            for decision in ledger::overrides(&ledger::recent(dir, session), &shape, now) {
                let mut entry = self.entry("override");
                entry.tool = Some(call.tool.clone());
                entry.shape = Some(shape.clone());
                entry.decisions.push(decision);
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
                revision: Some(d.revision.clone()).filter(|r| !r.is_empty()),
            })
            .collect();
        ledger::append(dir, &entry);
    }

    fn post_tool_use(
        &self,
        payload: &Value,
        call: &Call,
        records: &[Record],
        warnings: &[jouet_rules::Decision],
        ran: Option<Ran>,
    ) {
        let mut entry = self.entry("post-tool-use");
        // A run after a `cd` belongs to the repository it went to.
        let (tree, repo) = ran.map_or((None, None), |r| (r.tree, r.repo));
        if repo.is_some() {
            entry.repo = repo;
        }
        entry.tool = Some(call.tool.clone());
        entry.shape = call.command.as_deref().map(ledger::shape);
        entry.exit_code = call.exit_code;
        entry.duration_ms = payload.get("duration_ms").and_then(Value::as_u64);
        entry.output_bytes = output_bytes(payload);
        entry.tree = tree;
        entry.decisions = records.iter().map(decision).collect();
        entry
            .decisions
            .extend(warnings.iter().map(|d| ledger::Decision {
                rule: d.rule.clone(),
                action: "warn".into(),
                scope: None,
                revision: Some(d.revision.clone()).filter(|r| !r.is_empty()),
            }));
        self.append(entry);
    }
}

/// The rules for the session's directory, or for this process's.
fn load(payload: &Value) -> Option<Engine> {
    load_all(payload).map(|loaded| loaded.engine)
}

/// The rules, and the sets they come from.
fn load_all(payload: &Value) -> Option<rules::Loaded> {
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    let session = payload.get("session_id").and_then(Value::as_str);
    let loaded = rules::load_with(
        &cwd,
        &rules::Load {
            session,
            ..rules::Load::default()
        },
    );
    for error in &loaded.errors {
        eprintln!("jouet: {error}");
    }
    Some(loaded)
}

/// One line on rules and routines here that do not run until the user
/// trusts them or fetches them. It never asks the agent to trust them: the
/// rules are the ones that would be steering it.
fn untrusted_note(loaded: &rules::Loaded) -> Option<String> {
    let mut untrusted = Vec::new();
    let mut unfetched = Vec::new();
    for set in &loaded.sets {
        match set.status {
            Status::Untrusted | Status::Changed => {
                let rules = loaded
                    .engine
                    .rules()
                    .iter()
                    .filter(|r| r.origin.path().is_some_and(|p| set.holds(p)))
                    .count();
                let routines = jouet_routines::routine::luau_files(&set.routines_dir())
                    .map_or(0, |files| files.len());
                let changed = if set.status == Status::Changed {
                    ", changed since they were trusted"
                } else {
                    ""
                };
                untrusted.push(format!(
                    "{rules} rules and {routines} routines in {}{changed}",
                    set.root.display()
                ));
            }
            Status::Missing | Status::Mismatch => unfetched.push(set.label()),
            _ => {}
        }
    }
    let mut parts = Vec::new();
    if !untrusted.is_empty() {
        parts.push(format!(
            "this repository's rules and routines are not trusted, so they do not run ({}); trusting them is the user's decision, in their own terminal (`jouet trust`), not yours",
            untrusted.join("; ")
        ));
    }
    if !unfetched.is_empty() {
        parts.push(format!(
            "{} not fetched as pinned; `jouet plugin sync` fetches it",
            unfetched.join(", ")
        ));
    }
    (!parts.is_empty()).then(|| format!("jouet: {}.", parts.join("; ")))
}

fn evaluate(engine: &Engine, call: &Call) -> Outcome {
    let outcome = engine.evaluate(call);
    for error in &outcome.errors {
        eprintln!("jouet: {error}");
    }
    crate::share::record_gaps(engine);
    outcome
}

/// How `harness` waits for a long command in one call, for the primer. Claude
/// Code moves a Bash call past its 2-minute default timeout to the
/// background, and Codex's exec returns after its yield time; either way the
/// agent then spent a request on each check of it. In one benchmark run,
/// Codex polled a test run 13 times, 580k of its 1.87M input tokens.
fn wait_hint(harness: Harness) -> Option<&'static str> {
    match harness {
        Harness::Claude => Some(
            "Give a build or test that can take over 2 minutes a Bash `timeout` of 600000, so it ends in that call.",
        ),
        Harness::Codex => Some(
            "Wait for a long command in one call (a large yield_time_ms, or a loop on write_stdin until it exits), not a call per poll.",
        ),
        Harness::Cursor | Harness::Gemini | Harness::Copilot => None,
    }
}

/// About how many tokens the primer for `harness` in `cwd` is, at four
/// characters a token, without the notes that follow it only sometimes; 0
/// when the settings turn it off. `jouet stats` charges it to every request.
pub(crate) fn primer_tokens(settings: &Settings, cwd: &Path, harness: &str) -> u64 {
    let hooks = &settings.hooks;
    let Some(harness) = Harness::parse(harness).filter(|_| hooks.enabled && hooks.prime) else {
        return 0;
    };
    let primer = session_start(
        &rules::load(cwd).engine,
        hooks.steer,
        hooks.verify && hooks.gate_blocks(),
        harness,
        Some(cwd),
    );
    let chars = primer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .map_or(0, |s| s.chars().count());
    (chars as u64).div_ceil(4)
}

fn session_start(
    engine: &Engine,
    steer: bool,
    verify: bool,
    harness: Harness,
    cwd: Option<&Path>,
) -> Value {
    // Only the rules that deny: a warning costs the agent no call.
    let rules: Vec<(&str, Option<&str>)> = if steer {
        engine
            .rules()
            .iter()
            .filter(|r| {
                r.state == jouet_rules::State::Enabled
                    && r.event == Event::PreToolUse
                    && r.mode == jouet_rules::Mode::Deny
                    && r.primer
            })
            // A rule about project files not here never fires here.
            .filter(|r| engine.in_use_here(r))
            .map(|r| (r.name.as_str(), r.suggest()))
            .collect()
    } else {
        Vec::new()
    };
    let fallback = std::env::current_dir().ok();
    let cwd = cwd.or(fallback.as_deref());
    let affected = cwd.is_some_and(|cwd| !crate::affected::near(cwd).is_empty());
    let command = cwd
        .map(crate::project::affected_command)
        .unwrap_or_default();
    let (affected, command) = match command {
        crate::project::AffectedCommand::Default => (affected, None),
        crate::project::AffectedCommand::Configured(command) => (affected, Some(command)),
        crate::project::AffectedCommand::Broken => (false, None),
    };
    let mut context = guide::summary(
        &rules,
        verify,
        affected,
        command.as_deref(),
        wait_hint(harness),
    );
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
/// The longest a Bash call may run in Claude Code, past which it would move
/// to the background.
const LONG_RUN_MS: u64 = 600_000;

/// For Claude Code, a Bash call that runs a test, build or lint (what the
/// `checks` rule records) with no timeout of its own gets the longest, so it
/// ends in that call. Past the default two minutes it moves to the
/// background, and agents then spent a request on each look at it: three of
/// twelve in one benchmark run. The timeout goes through `updatedInput` with
/// no `permissionDecision`: Claude Code then applies the user's own
/// permission check to the updated call, so this works in every permission
/// mode and skips no prompt. The command itself is never changed.
fn with_long_timeout(
    engine: &Engine,
    payload: &Value,
    call: &Call,
    output: Option<Value>,
) -> Option<Value> {
    let Some(input) = payload.get("tool_input").and_then(Value::as_object) else {
        return output;
    };
    let timed = input.contains_key("timeout")
        || input.get("run_in_background").and_then(Value::as_bool) == Some(true);
    if call.tool != "Bash" || timed {
        return output;
    }
    // A check run is what the post-tool-use rules would record.
    let after = Call {
        event: Some(Event::PostToolUse),
        exit_code: Some(0),
        ..call.clone()
    };
    if engine.evaluate(&after).records.is_empty() {
        return output;
    }
    let mut updated = input.clone();
    updated.insert("timeout".into(), json!(LONG_RUN_MS));
    let mut out = output.unwrap_or_else(|| json!({}));
    let specific = out
        .as_object_mut()
        .expect("hook output is an object")
        .entry("hookSpecificOutput")
        .or_insert_with(|| json!({"hookEventName": "PreToolUse"}));
    if let Some(specific) = specific.as_object_mut() {
        specific.insert("updatedInput".into(), Value::Object(updated));
    }
    Some(out)
}

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

/// A decision's message, with the rule that made it.
fn tag(d: &jouet_rules::Decision) -> String {
    format!(
        "{} [jouet rule: {}]",
        d.message.as_deref().unwrap_or("jouet denied this command"),
        d.rule
    )
}

fn respond(outcome: &Outcome) -> Option<Value> {
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

/// Where a call's recorded runs ran, for the ledger: the first work tree
/// they ran in, hashed, and that repository's key.
struct Ran {
    tree: Option<String>,
    repo: Option<String>,
}

/// Runs the post-tool-use rules on a finished Bash command: what they
/// record and what they warn about. A command whose exit code is unknown, or
/// that runs in the background and so has not finished, gets nothing.
fn post_tool_use(engine: &Engine, payload: &Value, call: &Call) -> Outcome {
    let background = payload
        .get("tool_input")
        .and_then(|i| i.get("run_in_background"))
        .and_then(Value::as_bool)
        == Some(true);
    if call.tool != "Bash" || call.exit_code.is_none() || background {
        return Outcome::default();
    }
    let call = Call {
        event: Some(Event::PostToolUse),
        ..call.clone()
    };
    evaluate(engine, &call)
}

/// What the agent is told after a tool: a warning held from before it ran,
/// then the post-tool-use rules' warnings, each tagged with its rule.
fn after_tool(
    event: &str,
    held: Option<String>,
    warnings: &[jouet_rules::Decision],
) -> Option<Value> {
    let context: Vec<String> = held.into_iter().chain(warnings.iter().map(tag)).collect();
    if context.is_empty() {
        return None;
    }
    // Claude Code checks that the answer names the event it asked about.
    let name = if event == "post-tool-use-failure" {
        "PostToolUseFailure"
    } else {
        "PostToolUse"
    };
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": name,
            "additionalContext": context.join("\n"),
        }
    }))
}

/// Keeps the runs the post-tool-use rules recorded in the session's state
/// file, each with the tree it ran against; returns where the first ran.
fn record(payload: &Value, call: &Call, records: &[Record]) -> Option<Ran> {
    if records.is_empty() {
        return None;
    }
    let path = session_state_path(payload)?;
    // The command has finished, so the tree now is the one it ran against,
    // unless the command itself changed tracked files. Each run counts for
    // the work tree it ran in, after any `cd` before it.
    let cwd = session_dir(payload);
    let mut dirs: Vec<Option<&str>> = Vec::new();
    for record in records {
        if !dirs.contains(&record.dir.as_deref()) {
            dirs.push(record.dir.as_deref());
        }
    }
    let trees: Vec<(Option<&str>, String, Option<String>)> = dirs
        .into_iter()
        .map(|dir| {
            let at = run_dir(cwd.as_deref(), dir);
            let root = at.as_deref().map(verify::root_of).unwrap_or_default();
            let tree = jouet_rules::tree::work_tree(at.as_deref(), TREE_TIMEOUT).ok();
            (dir, root, tree)
        })
        .collect();
    let repo = (!trees[0].1.is_empty())
        .then(|| jouet_output::paths::repo_key(Path::new(&trees[0].1)))
        .flatten();
    // An hk check of a file list that names every changed file checked
    // what hk without files would have.
    let records: Vec<Record> = records
        .iter()
        .cloned()
        .map(|mut record| {
            if let Some(at) = run_dir(cwd.as_deref(), record.dir.as_deref()) {
                let script = call.command.as_deref().unwrap_or_default();
                verify::widen_hk_files(&mut record, &at, script, TREE_TIMEOUT);
            }
            record
        })
        .collect();
    // A failure whose output blames the machine (no D-Bus, a missing
    // system library, no network) is marked so: it does not hold up
    // finishing. Claude Code puts a failed call's output in `error`.
    let cause = (call.exit_code != Some(0))
        .then(|| payload.get("error").and_then(Value::as_str))
        .flatten()
        .and_then(|text| {
            let formats = cwd
                .as_deref()
                .map(|c| testlog::format::load(c).0)
                .unwrap_or_default();
            testlog::environment_cause(text, &formats)
        });
    let lead = agents::of_payload(payload).is_none();
    let _ = verify::update(&path, |state| {
        for (dir, root, tree) in &trees {
            state.by(root, lead);
            let here: Vec<Record> = records
                .iter()
                .filter(|r| r.dir.as_deref() == *dir)
                .cloned()
                .collect();
            let commands: Vec<String> = here.iter().map(|r| r.command.clone()).collect();
            let failed: Vec<String> = here
                .iter()
                .filter(|r| r.exit_code != 0)
                .map(|r| r.command.clone())
                .collect();
            state.record(root, here, tree.as_deref());
            if let Some(cause) = &cause {
                for command in &commands {
                    state.mark_environment(root, command, cause);
                }
            }
            mark_broken(state, root, &failed);
        }
    });
    let tree = trees.into_iter().next().and_then(|(_, _, tree)| tree);
    Some(Ran { tree, repo })
}

/// Keeps the tree before and after a call that only formats or commits
/// ([`verify::formats`]), so a run that passed before it counts after: a
/// formatter, or a commit's hooks fixing files, needs no second test run.
fn note_formatting(payload: &Value, call: &Call, started: bool) {
    let formats = call.tool == "Bash" && call.command.as_deref().is_some_and(verify::formats);
    let (true, Some(path), Some(cwd)) =
        (formats, session_state_path(payload), session_dir(payload))
    else {
        return;
    };
    let root = verify::root_of(&cwd);
    let tree = jouet_rules::tree::work_tree(Some(&cwd), TREE_TIMEOUT).ok();
    let lead = agents::of_payload(payload).is_none();
    let _ = verify::update(&path, |state| {
        state.by(&root, lead);
        let checks = state.checks_mut(&root);
        if started {
            checks.formatting_started(tree.as_deref());
        } else {
            checks.formatting_ended(tree.as_deref());
        }
    });
}

/// The directory the session's commands start in.
fn session_dir(payload: &Value) -> Option<PathBuf> {
    payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// Where a run recorded with `dir` ran, from the session's directory.
fn run_dir(cwd: Option<&Path>, dir: Option<&str>) -> Option<PathBuf> {
    match (cwd, dir) {
        (Some(cwd), Some(dir)) => Some(jouet_rules::resolve_dir(cwd, dir)),
        (None, Some(dir)) => Some(jouet_rules::resolve_dir(Path::new("."), dir)),
        (cwd, None) => cwd.map(Path::to_path_buf),
    }
}

/// A run of `jouet test`, recorded by `jouet test` itself, as `command`
/// (how it was called) exiting `exit_code` in `cwd`. The hooks cannot see
/// its exit status when its output is piped (`jouet test | tail` exits with
/// tail's), and Codex sends hooks none at all, so it keeps its own result:
/// in the session's state for the stop gate, and in the ledger for `pr
/// land`. It goes through the same post-tool-use
/// rules as a hook's run, so a rule that would not record it records
/// nothing here either.
/// Marks this `jouet test` process as running in the session's state, or,
/// with `running` false, clears the mark once it has ended: while it runs,
/// the stop gate does not hold up an agent waiting for it on what earlier
/// test runs found.
pub(crate) fn mark_own_run(cwd: &Path, command: &str, running: bool) {
    let settings = Settings::load();
    if !settings.hooks.enabled || !settings.hooks.verify {
        return;
    }
    let Some((path, lead)) = own_state() else {
        return;
    };
    let root = verify::root_of(cwd);
    let pid = std::process::id();
    let _ = verify::update(&path, |state| {
        if running {
            state.by(&root, lead);
            let command = command.to_string();
            state.start_run(&root, verify::Running { pid, command });
        } else {
            state.end_run(&root, pid);
        }
    });
}

/// Marks a `jouet test` run whose failures could not be compared with the
/// merge base, which could not run the command either: the stop gate asks
/// about them once, not at every turn, since nothing here can settle it.
pub(crate) fn mark_unverifiable(cwd: &Path, command: &str) {
    let settings = Settings::load();
    if !settings.hooks.enabled || !settings.hooks.verify {
        return;
    }
    let Some((path, lead)) = own_state() else {
        return;
    };
    let root = verify::root_of(cwd);
    let _ = verify::update(&path, |state| {
        state.by(&root, lead);
        state.mark_unverifiable(&root, command);
    });
}

/// The state file of the session this `jouet test` runs in, and whether its
/// lead ran it rather than a subagent. One the hooks cannot place counts as
/// a subagent's: a tree only it ran in is not held against the lead.
fn own_state() -> Option<(PathBuf, bool)> {
    let session = jouet_git::session::id_from_env()?;
    let lead = agents::caller(&session, "test").is_lead();
    Some((verify::state_path(&session)?, lead))
}

/// Records that a `jouet commit`'s hooks rewrote files, from the tree
/// `before` to `after`: what passed at `before` counts at `after`, as after a
/// formatter's call. Its call may run tests or `cd` first, which the hooks
/// cannot tell from a call that edits.
pub(crate) fn record_own_formatting(cwd: &Path, before: Option<&str>, after: Option<&str>) {
    let settings = Settings::load();
    let (Some(before), Some(after)) = (before, after) else {
        return;
    };
    if !settings.hooks.enabled || !settings.hooks.verify || before == after {
        return;
    }
    let Some((path, lead)) = own_state() else {
        return;
    };
    let root = verify::root_of(cwd);
    let _ = verify::update(&path, |state| {
        state.by(&root, lead);
        state.checks_mut(&root).reformatted(before, after);
    });
}

/// Records a `jouet test` run as the checks see it; `environment` is what
/// its output blames on the machine, when every failure does, `unrelated`
/// the failing tests when the change reaches none of them, `tests` the
/// failing tests its runner named, and `ran_tests` that it was a `--cmd`
/// run that may have tested something.
pub(crate) fn record_own_run(
    cwd: &Path,
    command: &str,
    exit_code: i32,
    environment: Option<&str>,
    unrelated: Option<&str>,
    tests: &[String],
    ran_tests: bool,
) {
    let settings = Settings::load();
    let hooks = &settings.hooks;
    if !hooks.enabled || !(hooks.verify || hooks.ledger) {
        return;
    }
    let call = Call {
        event: Some(Event::PostToolUse),
        harness: "jouet".into(),
        tool: "Bash".into(),
        command: Some(command.to_string()),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        exit_code: Some(exit_code),
        ..Call::default()
    };
    let mut records = rules::load(cwd).engine.evaluate(&call).records;
    // A `jouet test --cmd` of a command no rule knows, such as a project's
    // own check script, is a test the agent ran: its pass counts as a test
    // passing at these files, as a part of the suite. Not a run that only
    // listed tests or formatted files, and not a failure, since it may be
    // no test.
    if records.is_empty() && exit_code == 0 && ran_tests {
        records.push(Record {
            rule: "checks".into(),
            kind: jouet_rules::Kind::Test,
            command: command.to_string(),
            exit_code,
            full: false,
            selector: None,
            dir: None,
        });
    }
    if records.is_empty() {
        return;
    }
    let tree = jouet_rules::tree::work_tree(Some(cwd), TREE_TIMEOUT).ok();
    let session = jouet_git::session::id_from_env();
    if hooks.verify
        && let Some((path, lead)) = own_state()
    {
        let root = verify::root_of(cwd);
        let _ = verify::update(&path, |state| {
            state.by(&root, lead);
            state.record(&root, records.clone(), tree.as_deref());
            if let Some(cause) = environment {
                for record in &records {
                    state.mark_environment(&root, &record.command, cause);
                }
            }
            if let Some(tests) = unrelated {
                for record in &records {
                    state.mark_unrelated(&root, &record.command, tests);
                }
            }
            if !tests.is_empty() {
                for record in records.iter().filter(|r| r.exit_code != 0) {
                    state.mark_failed_tests(&root, &record.command, tests);
                }
            }
            let failed: Vec<String> = records
                .iter()
                .filter(|r| r.exit_code != 0)
                .map(|r| r.command.clone())
                .collect();
            mark_broken(state, &root, &failed);
        });
    }
    if hooks.ledger
        && let Some(dir) = ledger::dir()
    {
        let mut entry = Entry::new("jouet", session.as_deref(), "test-run");
        entry.repo = ledger::repo_key(cwd);
        entry.shape = Some(ledger::shape(command));
        entry.exit_code = Some(exit_code);
        entry.tree = tree;
        entry.decisions = records.iter().map(decision).collect();
        ledger::append(&dir, &entry);
    }
}

/// Keeps how long a Bash call that ran one check took, for `jouet cost`.
/// A call that ran several is not split between them, and `jouet test`
/// times each of its own jobs.
fn keep_cost(payload: &Value, call: &Call, records: &[Record], verified: bool) {
    let (Some(command), Some(cwd), Some(ms), Some(exit)) = (
        call.command.as_deref(),
        call.cwd.as_deref(),
        payload.get("duration_ms").and_then(Value::as_u64),
        call.exit_code,
    ) else {
        return;
    };
    if !one_pipeline(command) {
        return;
    }
    let cwd = Path::new(cwd);
    // Without verify, nothing evaluated the checks yet.
    let found;
    let records = if verified {
        records
    } else {
        found = checks(cwd, command);
        &found
    };
    if let [record] = records
        && record.dir.is_none()
        && !record.command.starts_with("jouet ")
    {
        crate::cost::record(cwd, &[crate::cost::Sample::of(record, ms, exit)]);
    }
}

/// Whether `command` is one pipeline, so its time is its check's: in `npm
/// ci && npm test`, the install would count as the test's, and a job sent
/// to the background is not timed at all. Redirections such as `2>&1` are
/// not separators. A separator inside quotes skips a call that could have
/// counted, which only loses a timing.
fn one_pipeline(command: &str) -> bool {
    let bare = command
        .replace("2>&1", "")
        .replace("1>&2", "")
        .replace(">&2", "")
        .replace("&>", "");
    !["&", "||", ";", "\n"].iter().any(|s| bare.contains(s))
}

/// The checks the rules here would record for `command` exiting 0: what
/// it builds, tests or lints.
pub(crate) fn checks(cwd: &Path, command: &str) -> Vec<Record> {
    let call = Call {
        event: Some(Event::PostToolUse),
        harness: "jouet".into(),
        tool: "Bash".into(),
        command: Some(command.to_string()),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        exit_code: Some(0),
        ..Call::default()
    };
    rules::load(cwd).engine.evaluate(&call).records
}

/// Marks each of `failed` that the trusted `.jouet/project.toml` lists as
/// failing before any change, as a failure that blames the machine is: it
/// does not hold up finishing, and the stop says it in a note.
fn mark_broken(state: &mut verify::State, root: &str, failed: &[String]) {
    if failed.is_empty() {
        return;
    }
    let Some(Ok(project)) = crate::project::trusted(Path::new(root)) else {
        return;
    };
    for command in failed {
        if let Some(why) = project.checks.broken(command) {
            state.mark_environment(
                root,
                command,
                &format!("fails here before any change: {why} (.jouet/project.toml)"),
            );
        }
    }
}

/// A recorded run, as the ledger keeps it: its kind and scope, never what
/// it selected.
fn decision(r: &Record) -> ledger::Decision {
    ledger::Decision {
        rule: r.rule.clone(),
        action: format!("record:{}", r.kind.name()),
        scope: Some(if r.full { "full" } else { "partial" }.into()),
        revision: None,
    }
}

/// How many files the work tree tracks, for a search a `min_tracked_files`
/// matcher could be about: the Grep tool, or a command running grep or rg.
/// Other calls do not pay for `git ls-files`. After a `cd`, the search runs
/// somewhere else, so the count is unknown and the matcher holds.
fn tracked_files(call: &Call) -> Option<u32> {
    let words: Vec<&str> = call
        .command
        .as_deref()
        .map(|c| c.split(|ch: char| !ch.is_ascii_alphanumeric()).collect())
        .unwrap_or_default();
    let search = call.tool == "Grep" || words.iter().any(|w| *w == "grep" || *w == "rg");
    if !search || words.iter().any(|w| *w == "cd" || *w == "pushd") {
        return None;
    }
    let cwd = call.cwd.as_deref().map(Path::new);
    jouet_rules::tree::tracked_files(cwd, TREE_TIMEOUT).ok()
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
/// Whether the change in the work tree at `root` is a small fix: at most
/// [`verify::SMALL_FIX_LINES`] lines of code differ from the merge base with
/// its base branch ([`crate::affected::changed_code_lines`]).
fn small_fix(root: &Path) -> bool {
    crate::affected::changed_code_lines(root).is_some_and(|lines| lines <= verify::SMALL_FIX_LINES)
}

/// What a stop decided.
#[derive(Debug, Default)]
struct Stopped {
    /// The hook's stdout.
    output: Option<Value>,
    /// What each reason the gate found now is about ([`verify::Block::about`]),
    /// whether it blocked for them or only observed them.
    reasons: Vec<String>,
    /// Whether hk's merged stop hook blocked.
    hk_blocked: bool,
}

/// The stop: `gate` is None with `[hooks] verify` off, else whether the
/// gate blocks for its reasons (`[hooks] gate = "block"`) or only finds
/// them, for the ledger to record. hk's merged stop hook (`hk_stop`) blocks
/// either way: its verdict is hk's.
fn stop(harness: Harness, payload: &Value, gate: Option<bool>, hk_stop: &[String]) -> Stopped {
    let verify = gate.is_some();
    // Jobs left running are a note, never a reason to block.
    let mut note = if verify { jobs_note(payload) } else { None };
    let as_note = |note: Option<String>| Stopped {
        output: note.map(|note| json!({ "systemMessage": note })),
        ..Stopped::default()
    };
    // Block at most once in a row: after one nudge, the agent's answer
    // stands, so a check that cannot pass never traps it.
    if payload.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return as_note(note);
    }
    let mut state = match harness {
        Harness::Claude | Harness::Cursor | Harness::Gemini | Harness::Copilot => {
            session_state_path(payload).map(|p| State::load(&p))
        }
        Harness::Codex => payload
            .get("transcript_path")
            .and_then(Value::as_str)
            .map(|t| verify::codex_commands(t.as_ref()))
            .filter(|commands| !commands.is_empty())
            .and_then(|commands| Some(codex_state(&load(payload)?, payload, commands))),
    }
    .unwrap_or_default();
    state.prune_gone();
    // Failures blamed on the machine hold nothing up; the person hears of
    // each once.
    let environment = if verify {
        state.environment_notes()
    } else {
        Vec::new()
    };
    if !environment.is_empty() {
        note = Some(
            note.into_iter()
                .chain(environment)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if harness != Harness::Codex
            && let Some(path) = session_state_path(payload)
        {
            let _ = verify::update(&path, State::noted);
        }
    }
    let tree = std::cell::OnceCell::new();
    let now = || tree.get_or_init(|| work_tree(payload)).clone();
    let home = session_dir(payload)
        .as_deref()
        .map(verify::root_of)
        .unwrap_or_default();
    state.set_aside_settled(
        &home,
        jouet_output::time::now(),
        |root| jouet_rules::tree::uncommitted(Path::new(root), TREE_TIMEOUT).ok(),
        |root| {
            let root = Path::new(root);
            crate::affected::base(root, None).is_ok_and(|base| {
                crate::affected::unchanged_since(root, &base.merge_base).unwrap_or(false)
            })
        },
    );
    // The session's own tree is hashed once; another is hashed only when
    // its checks need it.
    let tree_of = |root: &str| {
        if root == home {
            now()
        } else if root.is_empty() {
            None
        } else {
            jouet_rules::tree::work_tree(Some(Path::new(root)), TREE_TIMEOUT).ok()
        }
    };

    let hk_repo =
        |root: &str| !root.is_empty() && crate::diag::hk_config(Path::new(root)).is_some();
    let scope = |root: &str| verify::Scope {
        affected: !root.is_empty() && !crate::affected::near(Path::new(root)).is_empty(),
        // Only a tree whose newest test run passed is asked about after
        // an edit, so only there is the change measured.
        small_fix: !root.is_empty()
            && state
                .checks(root)
                .is_some_and(verify::Checks::last_test_passed)
            && small_fix(Path::new(root)),
    };
    let block = if verify {
        state.block(&home, tree_of, hk_repo, scope)
    } else {
        None
    };
    // Kept so the next turn's stop, at the same tree with no run since,
    // does not raise it again. Codex's runs are read again from the
    // transcript at each stop, so there is nothing to keep it with.
    if let Some(block) = &block
        && harness != Harness::Codex
        && let Some(path) = session_state_path(payload)
    {
        let _ = verify::update(&path, |state| state.raise(block));
    }
    // Observed, a reason is kept as raised all the same, so each is
    // recorded once, where the gate would have blocked for it.
    let reasons = block.as_ref().map(verify::Block::about).unwrap_or_default();
    let jouet = block.map(|b| b.reason).filter(|_| gate == Some(true));
    let hk_passed = state.checks(&home).is_some_and(|c| c.hk_passed_at(now));
    let hk = if hk_stop.is_empty() || hk_passed {
        None
    } else {
        run_hk_stop(hk_stop, payload)
    };
    let hk_blocked = hk.is_some();
    let reason = match (jouet, hk) {
        (None, None) => {
            return Stopped {
                reasons,
                ..as_note(note)
            };
        }
        (Some(one), None) => one,
        (None, Some(hk)) => format!("{hk} [hk]"),
        (Some(jouet), Some(hk)) => format!("{jouet}\n\nhk check: {hk} [hk]"),
    };
    let reason = match &note {
        Some(note) => format!("{reason}\n{note}"),
        None => reason,
    };
    Stopped {
        output: Some(json!({ "decision": "block", "reason": reason })),
        reasons,
        hk_blocked,
    }
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
fn codex_state(engine: &Engine, payload: &Value, commands: Vec<verify::Ran>) -> State {
    let mut state = State::default();
    let cwd = session_dir(payload);
    let mut failed = std::collections::BTreeMap::<String, Vec<String>>::new();
    for verify::Ran {
        command,
        exit_code,
        at: when,
        output,
        cwd: command_cwd,
    } in commands
    {
        let call = Call {
            event: Some(Event::PostToolUse),
            harness: "codex".into(),
            tool: "Bash".into(),
            command: Some(command),
            path: None,
            cwd: command_cwd.clone(),
            exit_code: Some(exit_code),
            ..Call::default()
        };
        // The transcript does not say what tree each command saw.
        for record in evaluate(engine, &call).records {
            let at = run_dir(
                command_cwd.as_deref().map(Path::new).or(cwd.as_deref()),
                record.dir.as_deref(),
            );
            let root = at.as_deref().map(verify::root_of).unwrap_or_default();
            // A failure whose output blames the machine is marked so, as the
            // other harnesses' failures are, judged with the formats of the
            // work tree it ran in.
            let formats = || {
                at.as_deref()
                    .map(|dir| testlog::format::load(dir).0)
                    .unwrap_or_default()
            };
            let cause = output
                .as_deref()
                .and_then(|text| testlog::environment_cause(text, &formats()));
            // The failing tests it named, so a later filter that picks
            // their paths can clear it, as with the other harnesses.
            let tests = output
                .as_deref()
                .filter(|_| record.exit_code != 0)
                .and_then(|text| {
                    let dir = at.as_deref().unwrap_or(Path::new(&root));
                    super::test::failed_test_names(text, &formats(), dir)
                })
                .unwrap_or_default();
            let recorded = record.command.clone();
            if record.exit_code != 0 {
                failed
                    .entry(root.clone())
                    .or_default()
                    .push(record.command.clone());
            }
            state.record(&root, [record], None);
            if let Some(cause) = &cause {
                state.mark_environment(&root, &recorded, cause);
            }
            if !tests.is_empty() {
                state.mark_failed_tests(&root, &recorded, &tests);
            }
            // Read again at every stop: dated now, a run in another work
            // tree would never be left behind (see `set_aside_settled`).
            if let Some(when) = when {
                state.ran_at(&root, when);
            }
        }
    }
    // Once per work tree: each reads its project file.
    for (root, commands) in &failed {
        mark_broken(&mut state, root, commands);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_bytes_are_what_the_tool_gave_the_agent() {
        let bash =
            json!({"tool_response": {"stdout": "abc", "stderr": "de", "interrupted": false}});
        assert_eq!(output_bytes(&bash), Some(5));
        assert_eq!(output_bytes(&json!({"tool_response": "four"})), Some(4));
        // A Read or Edit result is not a shell's output, and none is none.
        assert_eq!(output_bytes(&json!({"tool_response": {"file": {}}})), None);
        assert_eq!(output_bytes(&json!({})), None);
    }
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
    fn a_check_run_gets_a_long_timeout_without_a_permission_decision() {
        let engine = engine(Vec::new());
        let payload = json!({"tool_name": "Bash", "tool_input": {"command": "cargo test"}, "permission_mode": "default"});
        let call = Call::from_hook(&payload, "claude");
        let out = with_long_timeout(&engine, &payload, &call, None).unwrap();
        let specific = &out["hookSpecificOutput"];
        assert_eq!(
            specific["updatedInput"],
            json!({"command": "cargo test", "timeout": LONG_RUN_MS})
        );
        // No allow, so the user's own permission check still applies.
        assert!(specific.get("permissionDecision").is_none());
        assert!(specific.get("permissionDecisionReason").is_none());
    }

    #[test]
    fn a_post_tool_use_warning_reaches_the_agent_after_the_call() {
        let engine = engine(Vec::new());
        let payload = json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "gh pr checks 12"},
            "error": "Exit code 8\nSome checks are still pending",
        });
        let call = Call::from_hook(&payload, "claude");
        let outcome = post_tool_use(&engine, &payload, &call);
        let out = after_tool("post-tool-use-failure", None, &outcome.warnings).unwrap();
        let output = &out["hookSpecificOutput"];
        assert_eq!(output["hookEventName"], "PostToolUseFailure");
        let context = output["additionalContext"].as_str().unwrap();
        assert!(context.contains("`jouet wait ci`"), "{context}");
        assert!(context.ends_with("[jouet rule: ci-pending]"), "{context}");
        // A warning held from before the call comes first.
        let out = after_tool("post-tool-use", Some("held".into()), &outcome.warnings).unwrap();
        let context = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.starts_with("held\n"), "{context}");
        assert_eq!(after_tool("post-tool-use", None, &[]), None);
        // A command in the background has not finished.
        let background = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "gh pr checks 12", "run_in_background": true},
            "exit_code": 8,
        });
        let call = Call::from_hook(&background, "claude");
        assert!(
            post_tool_use(&engine, &background, &call)
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn tracked_files_are_counted_only_for_a_search_where_it_runs() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(repo.path().join("a.rs"), "fn a() {}\n").unwrap();
        git(&["add", "a.rs"]);
        let call = |command: &str| Call {
            tool: "Bash".into(),
            command: Some(command.into()),
            cwd: Some(repo.path().to_string_lossy().into_owned()),
            ..Call::default()
        };
        assert_eq!(tracked_files(&call("rg TODO")), Some(1));
        // Not a search: git is not asked.
        assert_eq!(tracked_files(&call("ls -la")), None);
        // After a `cd`, the search runs somewhere else.
        assert_eq!(tracked_files(&call("cd .. && rg TODO")), None);
    }

    fn ran(command: &str, exit_code: i32) -> verify::Ran {
        verify::Ran {
            command: command.into(),
            exit_code,
            at: None,
            output: None,
            cwd: None,
        }
    }

    #[test]
    fn codex_commands_keep_the_time_they_ran() {
        // Replayed at every stop, a run keeps its transcript time, so a
        // work tree left an hour ago can be set aside.
        let other = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(other.path())
            .status()
            .unwrap();
        let root = verify::root_of(other.path());
        let command = format!("cd {root} && cargo test --workspace");
        let state = codex_state(
            &engine(Vec::new()),
            &json!({}),
            vec![verify::Ran {
                at: Some(1_000),
                ..ran(&command, 101)
            }],
        );
        assert_eq!(state.checks(&root).and_then(|c| c.last_run), Some(1_000));
    }

    #[test]
    fn codex_replay_lets_a_filter_on_a_failed_tests_path_clear_it() {
        // `testlog` reruns `testlog::tests::a_summary`, which only the failed
        // run's output names.
        let repo = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        let root = verify::root_of(repo.path());
        let output = "running 1 test\ntest testlog::tests::a_summary ... FAILED\n\nfailures:\n\n---- testlog::tests::a_summary stdout ----\nboom\n\nfailures:\n    testlog::tests::a_summary\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        let at = |command: &str, exit_code, output: Option<&str>| verify::Ran {
            cwd: Some(root.clone()),
            output: output.map(str::to_string),
            ..ran(command, exit_code)
        };
        let replay = |output| {
            codex_state(
                &engine(Vec::new()),
                &json!({}),
                vec![
                    at("cargo test a_summary", 101, output),
                    at("cargo test testlog", 0, None),
                ],
            )
        };
        assert!(
            replay(Some(output))
                .block(&root, |_| None, |_| false, |_| true)
                .is_none()
        );
        assert!(
            replay(None)
                .block(&root, |_| None, |_| false, |_| true)
                .is_some()
        );
    }

    #[test]
    fn codex_replay_records_pnpm_failures_without_a_working_directory() {
        let state = codex_state(
            &engine(Vec::new()),
            &json!({}),
            vec![ran("pnpm test", 1), ran("pnpm test", 0)],
        );
        assert!(state.block("", |_| None, |_| false, |_| true).is_some());
    }

    #[test]
    fn codex_replay_attributes_a_run_to_its_recorded_working_directory() {
        let repo = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        let root = verify::root_of(repo.path());
        let command = verify::Ran {
            cwd: Some(root.clone()),
            ..ran("pnpm test", 1)
        };
        let state = codex_state(&engine(Vec::new()), &json!({}), vec![command]);
        assert!(state.block(&root, |_| None, |_| false, |_| true).is_some());
    }

    #[test]
    fn codex_file_uri_replay_records_and_clears_the_right_pnpm_failure() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("project space");
        std::fs::create_dir(&repo).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap();
        let root = verify::root_of(&repo);
        let encoded = repo
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20");
        let uri = format!(
            "{}{encoded}",
            if cfg!(windows) { "file:///" } else { "file://" }
        );
        let event = |exit_code| {
            json!({"payload": {"type": "item_completed", "item": {
            "type": "CommandExecution", "command": ["pnpm", "test"], "exit_code": exit_code, "cwd": uri
        }}}).to_string()
        };
        let transcript = parent.path().join("rollout.jsonl");
        std::fs::write(&transcript, event(1)).unwrap();
        let state = codex_state(
            &engine(Vec::new()),
            &json!({}),
            verify::codex_commands(&transcript),
        );
        assert!(state.block(&root, |_| None, |_| false, |_| true).is_some());
        std::fs::write(&transcript, format!("{}\n{}", event(1), event(0))).unwrap();
        let state = codex_state(
            &engine(Vec::new()),
            &json!({}),
            verify::codex_commands(&transcript),
        );
        assert!(state.block(&root, |_| None, |_| false, |_| true).is_none());
    }

    #[test]
    fn codex_commands_replay_through_the_post_tool_use_rules() {
        let engine = engine(Vec::new());
        let failing = codex_state(
            &engine,
            &json!({}),
            vec![ran("cargo test --workspace", 101), ran("ls", 0)],
        );
        assert!(failing.block("", |_| None, |_| false, |_| true).is_some());
        let fixed = codex_state(
            &engine,
            &json!({}),
            vec![
                ran("cargo test --workspace", 101),
                ran("cargo test --workspace", 0),
            ],
        );
        assert_eq!(fixed.block("", |_| None, |_| false, |_| true), None);
        // A partial pass leaves the full failure standing.
        let partial = codex_state(
            &engine,
            &json!({}),
            vec![
                ran("cargo test --workspace", 101),
                ran("cargo test -p jouet --lib", 0),
            ],
        );
        let why = partial
            .block("", |_| None, |_| false, |_| true)
            .unwrap()
            .reason;
        assert!(why.contains("only a full test run clears it"), "{why}");
        // A refusal, such as `diag check` with no hk config, is not a
        // failed lint, and does not clear one either.
        let refused = codex_state(
            &engine,
            &json!({}),
            vec![ran("/usr/bin/zsh -lc 'jouet diag check a.py'", 4)],
        );
        assert_eq!(refused.block("", |_| None, |_| false, |_| true), None);
        let still = codex_state(
            &engine,
            &json!({}),
            vec![
                ran("jouet diag check a.py", 1),
                ran("jouet diag check a.py", 4),
            ],
        );
        assert!(still.block("", |_| None, |_| false, |_| true).is_some());
    }

    #[test]
    fn winding_notes_follow_the_primer() {
        let primer = session_start(&engine(Vec::new()), true, true, Harness::Claude, None);
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
    fn only_a_call_that_is_one_pipeline_is_timed_as_its_check() {
        assert!(one_pipeline("cargo test 2>&1 | tail -20"));
        assert!(one_pipeline("mise x -- cargo test -p a"));
        assert!(!one_pipeline("npm ci && npm test"));
        assert!(!one_pipeline("cargo build; cargo test"));
        assert!(!one_pipeline("cargo test &"));
        assert!(!one_pipeline("cargo fmt\ncargo test"));
    }

    #[test]
    fn the_primer_names_the_active_rules() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(Vec::new());
        engine.set_probes(jouet_rules::Probes::new(
            Some(dir.path().to_path_buf()),
            None,
            false,
        ));
        let out = session_start(&engine, true, true, Harness::Claude, None);
        // The slow checks are a session-start note, read from the notes of
        // the repository the session is in, never part of the primer.
        let context = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        // How to wait for a long run in one call, for this harness.
        assert!(context.contains("a Bash `timeout` of 600000"), "{context}");
        let codex = session_start(&engine, true, true, Harness::Codex, None);
        let codex = codex["hookSpecificOutput"]["additionalContext"].to_string();
        assert!(codex.contains("loop on write_stdin"), "{codex}");
        assert!(!codex.contains("Bash `timeout`"), "{codex}");
        assert!(context.contains("git-stash"), "{context}");
        assert!(
            context.contains("python-edit (use `jouet edit`)"),
            "{context}"
        );
        // Every built-in deny rule, not a sample, fits the primer's budget.
        // It grew from 1200 to fit `jouet test`'s usage: about 40 tokens a
        // request, where the `agent-help test` and `commit` lookups it saves
        // printed 5.9 kB once each. Then to 1500 for `--baseline`: without it,
        // Codex changed the environment, outside its checkout, to make
        // failures it had not caused pass, in each of six benchmark runs.
        // Then to 1600 for waiting in one call: about 25 tokens a request,
        // where each check on a long run in the background cost a request
        // of 40k tokens or more. Then to 1700 for listing threads and for
        // committing in the test run's call: in the benchmark's short tasks,
        // agents guessed `pr threads list` and committed in a call of its
        // own, a request of about 20k tokens each. Then to 1900 for finding
        // code with `jouet search` and `jouet read --symbol`: in the
        // benchmark's usage task agents spent 14 to 18 requests on grep and
        // sed and never used them; named here, they did, and the median run
        // fell from 627k input tokens to 376k. Then to 2000 for reading
        // several definitions in one call: on zod, agents made five reads of
        // one file, at ~35k tokens of context each, where a plain agent
        // chained one sed.
        assert!(context.len() <= 2000, "{} bytes:\n{context}", context.len());
        // Only deny rules: a warning costs the agent no call, and neither
        // does naming one that denies on content (`primer = false`).
        assert!(!context.contains("large-read"), "{context}");
        assert!(!context.contains("committed-secrets"), "{context}");
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
