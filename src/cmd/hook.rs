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
    let harness = args.harness.as_deref().and_then(Harness::parse);
    let result = std::panic::catch_unwind(|| {
        let harness = harness?;
        let event = args.event.as_deref()?;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok()?;
        let payload: Value = serde_json::from_str(&input).ok()?;
        let payload = harness::normalize(harness, event, payload);
        if harness == Harness::Claude && cursor_runs_its_own(&payload) {
            return None;
        }
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
            let loaded = load_all(payload).filter(|_| hooks.prime)?;
            let primer = session_start(&loaded.engine, hooks.steer, hooks.verify, harness);
            let cwd = payload.get("cwd").and_then(Value::as_str).map(Path::new);
            let mut notes = wind_notes(settings, log.session, log.repo.as_deref(), cwd);
            notes.extend(untrusted_note(&loaded));
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
                remember(&call, log.session);
            }
            let (records, ran) = if hooks.verify {
                record(payload, &call)
            } else {
                (Vec::new(), None)
            };
            log.post_tool_use(payload, &call, &records, ran);
            keep_cost(payload, &call, &records);
            // A run of a mise task jouet added, for `jouet unwind`.
            if settings.wind.enabled
                && let (Some(command), Some(cwd)) = (call.command.as_deref(), call.cwd.as_deref())
                && command.contains("mise")
                && let Some(paths) = wind::Paths::locate()
            {
                wind::tasks::record_use(&paths, Path::new(cwd), command, crate::wind::now());
            }
            if hooks.graph {
                super::graph::reindex_after_tool(payload, &call);
            }
            // A warning the pre-tool-use hook held for this call.
            if !harness::defers_warnings(harness) {
                return None;
            }
            let path = pending_path(payload)?;
            let context = std::fs::read_to_string(&path).ok()?;
            let _ = std::fs::remove_file(&path);
            Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": context,
                }
            }))
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

    fn post_tool_use(&self, payload: &Value, call: &Call, records: &[Record], ran: Option<Ran>) {
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
        entry.tree = tree;
        entry.decisions = records.iter().map(decision).collect();
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
    let loaded = rules::load(&cwd);
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

fn session_start(engine: &Engine, steer: bool, verify: bool, harness: Harness) -> Value {
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
    let affected = std::env::current_dir().is_ok_and(|cwd| !crate::affected::near(&cwd).is_empty());
    let mut context = guide::summary(&rules, verify, affected, wait_hint(harness));
    if let Some(line) = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::cost::primer_line(&crate::cost::Costs::load(&cwd)))
    {
        context.push_str(&line);
    }
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

/// For Claude Code in `bypassPermissions` mode, a Bash call that runs a
/// test, build or lint (what the `checks` rule records) with no timeout of
/// its own gets the longest, so it ends in that call. Past the default two
/// minutes it moves to the background, and agents then spent a request on
/// each look at it: three of twelve in one benchmark run. Only in that mode,
/// since `updatedInput` needs `permissionDecision: "allow"`, which in any
/// other would skip the user's prompt; the command itself is never changed.
fn with_long_timeout(
    engine: &Engine,
    payload: &Value,
    call: &Call,
    output: Option<Value>,
) -> Option<Value> {
    let bypass =
        payload.get("permission_mode").and_then(Value::as_str) == Some("bypassPermissions");
    let Some(input) = payload.get("tool_input").and_then(Value::as_object) else {
        return output;
    };
    let timed = input.contains_key("timeout")
        || input.get("run_in_background").and_then(Value::as_bool) == Some(true);
    if !bypass || call.tool != "Bash" || timed {
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
        specific.insert("permissionDecision".into(), json!("allow"));
        specific.insert(
            "permissionDecisionReason".into(),
            json!("jouet: a 10-minute timeout, so this run ends in this call"),
        );
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

/// Where a call's recorded runs ran, for the ledger: the first work tree
/// they ran in, hashed, and that repository's key.
struct Ran {
    tree: Option<String>,
    repo: Option<String>,
}

/// Runs the post-tool-use rules on a finished Bash command and keeps what
/// they record in the session's state file, each with the tree it ran
/// against; returns them and where the first ran.
fn record(payload: &Value, call: &Call) -> (Vec<Record>, Option<Ran>) {
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
    // unless the command itself changed tracked files. Each run counts for
    // the work tree it ran in, after any `cd` before it.
    let cwd = session_dir(payload);
    let mut dirs: Vec<Option<&str>> = Vec::new();
    for record in &records {
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
    let _ = verify::update(&path, |state| {
        for (dir, root, tree) in &trees {
            let here = records.iter().filter(|r| r.dir.as_deref() == *dir).cloned();
            state.record(root, here, tree.as_deref());
        }
    });
    let tree = trees.into_iter().next().and_then(|(_, _, tree)| tree);
    (records, Some(Ran { tree, repo }))
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
    let Some(path) = jouet_git::session::id_from_env()
        .as_deref()
        .and_then(verify::state_path)
    else {
        return;
    };
    let root = verify::root_of(cwd);
    let pid = std::process::id();
    let _ = verify::update(&path, |state| {
        if running {
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
    let Some(path) = jouet_git::session::id_from_env()
        .as_deref()
        .and_then(verify::state_path)
    else {
        return;
    };
    let root = verify::root_of(cwd);
    let _ = verify::update(&path, |state| state.mark_unverifiable(&root, command));
}

pub(crate) fn record_own_run(cwd: &Path, command: &str, exit_code: i32) {
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
    let records = rules::load(cwd).engine.evaluate(&call).records;
    if records.is_empty() {
        return;
    }
    let tree = jouet_rules::tree::work_tree(Some(cwd), TREE_TIMEOUT).ok();
    let session = jouet_git::session::id_from_env();
    if hooks.verify
        && let Some(path) = session.as_deref().and_then(verify::state_path)
    {
        let root = verify::root_of(cwd);
        let _ = verify::update(&path, |state| {
            state.record(&root, records.clone(), tree.as_deref());
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
fn keep_cost(payload: &Value, call: &Call, records: &[Record]) {
    let ([record], Some(cwd), Some(ms), Some(exit)) = (
        records,
        call.cwd.as_deref(),
        payload.get("duration_ms").and_then(Value::as_u64),
        call.exit_code,
    ) else {
        return;
    };
    if record.dir.is_none() && !record.command.starts_with("jouet ") {
        crate::cost::record(Path::new(cwd), &[crate::cost::Sample::of(record, ms, exit)]);
    }
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

/// A recorded run, as the ledger keeps it: its kind and scope, never what
/// it selected.
fn decision(r: &Record) -> ledger::Decision {
    ledger::Decision {
        rule: r.rule.clone(),
        action: format!("record:{}", r.kind.name()),
        scope: Some(if r.full { "full" } else { "partial" }.into()),
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
fn stop(harness: Harness, payload: &Value, verify: bool, hk_stop: &[String]) -> Option<Value> {
    // Jobs left running are a note, never a reason to block.
    let note = if verify { jobs_note(payload) } else { None };
    let as_note = |note: Option<String>| note.map(|note| json!({ "systemMessage": note }));
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
    let tree = std::cell::OnceCell::new();
    let now = || tree.get_or_init(|| work_tree(payload)).clone();
    let home = session_dir(payload)
        .as_deref()
        .map(verify::root_of)
        .unwrap_or_default();
    state.set_aside_settled(&home, jouet_output::time::now(), |root| {
        jouet_rules::tree::uncommitted(Path::new(root), TREE_TIMEOUT).ok()
    });
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
    let affected =
        |root: &str| !root.is_empty() && !crate::affected::near(Path::new(root)).is_empty();
    let block = if verify {
        state.block(&home, tree_of, hk_repo, affected)
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
    let jouet = block.map(|b| b.reason);
    let hk_passed = state.checks(&home).is_some_and(|c| c.hk_passed_at(now));
    let hk = if hk_stop.is_empty() || hk_passed {
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
fn codex_state(engine: &Engine, payload: &Value, commands: Vec<verify::Ran>) -> State {
    let mut state = State::default();
    let cwd = session_dir(payload);
    for verify::Ran {
        command,
        exit_code,
        at: when,
    } in commands
    {
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
        for record in evaluate(engine, &call).records {
            let at = run_dir(cwd.as_deref(), record.dir.as_deref());
            let root = at.as_deref().map(verify::root_of).unwrap_or_default();
            state.record(&root, [record], None);
            // Read again at every stop: dated now, a run in another work
            // tree would never be left behind (see `set_aside_settled`).
            if let Some(when) = when {
                state.ran_at(&root, when);
            }
        }
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
        let primer = session_start(&engine(Vec::new()), true, true, Harness::Claude);
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
        let out = session_start(&engine(Vec::new()), true, true, Harness::Claude);
        let context = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        // How to wait for a long run in one call, for this harness.
        assert!(context.contains("a Bash `timeout` of 600000"), "{context}");
        let codex = session_start(&engine(Vec::new()), true, true, Harness::Codex);
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
        // own, a request of about 20k tokens each.
        assert!(context.len() <= 1700, "{} bytes:\n{context}", context.len());
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
