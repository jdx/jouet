//! jouet's rule engine: rules that read the command an agent is about to
//! run, or has just run, and decide what to do about it.
//!
//! A rule is configuration: a `[[rule]]` table in a TOML file ([`decl`])
//! with a matcher, a message and its examples. Matching is plain Rust over
//! the shell tokenizer's view of the command ([`matcher`]), so a rule that
//! only matches never starts a Luau VM. A rule with logic a matcher cannot
//! express names a Luau `script`, which runs in a sandboxed VM whose only way
//! out is the `jouet` module: the tool call it is given, a shell-aware view
//! of its commands, helpers for matching them, and the actions `deny`,
//! `warn`, `allow` and `record`. The VM starts the first time a script runs.
//! The built-in rules are written in the same format as anyone else's.
//!
//! A rule written as a whole `.luau` file, with a `--- why:` header, still
//! loads, but is deprecated ([`LUAU_RULES_REMOVED_IN`]).
//!
//! Every failure is contained: a rule that does not load, a script that
//! throws, runs past its time or instruction budget or returns something
//! unexpected is reported in [`Outcome::errors`] and treated as having said
//! nothing.
//!
//! Rules can also ask about jouet's state through read-only probes
//! ([`probe`]). Routines, which compose jouet's primitives, are
//! jouet-routines'.

mod cache;
mod decl;
mod examples;
mod glob;
mod header;
mod json;
mod matcher;
pub mod probe;
pub mod shell;
pub mod tree;
mod trust;

pub use decl::{SCHEMA_URL, schema, valid_name};
pub use examples::{Case, Example, Expect};
pub use header::{Mode, Scope};
pub use probe::Probes;
pub use trust::{Trust, TrustStore};

use mlua::{Lua, Table, Value, Variadic, VmState};
use serde::Serialize;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The Luau type definitions of the `jouet` module, for type-checking rules.
pub const TYPES: &str = include_str!("../jouet.d.luau");

const PRELUDE: &str = include_str!("prelude.luau");

/// The built-in rules, embedded in the binary, in the order they run.
const BUILTIN: &[(&str, &str)] = &[
    ("sleep-poll", include_str!("../rules/sleep-poll.toml")),
    ("ci-rerun", include_str!("../rules/ci-rerun.toml")),
    (
        "review-threads",
        include_str!("../rules/review-threads.toml"),
    ),
    ("python-edit", include_str!("../rules/python-edit.toml")),
    ("git-add-all", include_str!("../rules/git-add-all.toml")),
    ("git-stash", include_str!("../rules/git-stash.toml")),
    ("no-verify", include_str!("../rules/no-verify.toml")),
    ("force-push", include_str!("../rules/force-push.toml")),
    (
        "reset-to-remote",
        include_str!("../rules/reset-to-remote.toml"),
    ),
    (
        "rebase-merged-base",
        include_str!("../rules/rebase-merged-base.toml"),
    ),
    ("print-secrets", include_str!("../rules/print-secrets.toml")),
    ("secret-files", include_str!("../rules/secret-files.toml")),
    (
        "committed-secrets",
        include_str!("../rules/committed-secrets.toml"),
    ),
    ("push-untested", include_str!("../rules/push-untested.toml")),
    ("scoped-lint", include_str!("../rules/scoped-lint.toml")),
    (
        "affected-tests",
        include_str!("../rules/affected-tests.toml"),
    ),
    (
        "piped-test-output",
        include_str!("../rules/piped-test-output.toml"),
    ),
    // Before the job rules: a daemon the project declares is pitchfork's.
    (
        "pitchfork-start",
        include_str!("../rules/pitchfork-start.toml"),
    ),
    (
        "pitchfork-stop",
        include_str!("../rules/pitchfork-stop.toml"),
    ),
    (
        "pitchfork-logs",
        include_str!("../rules/pitchfork-logs.toml"),
    ),
    ("job-start", include_str!("../rules/job-start.toml")),
    ("job-ready", include_str!("../rules/job-ready.toml")),
    ("job-logs", include_str!("../rules/job-logs.toml")),
    ("job-stop", include_str!("../rules/job-stop.toml")),
    ("claimed-path", include_str!("../rules/claimed-path.toml")),
    ("checks", include_str!("../rules/checks.toml")),
    ("reread", include_str!("../rules/reread.toml")),
    ("large-read", include_str!("../rules/large-read.toml")),
    (
        "unbudgeted-search",
        include_str!("../rules/unbudgeted-search.toml"),
    ),
    (
        "unbounded-query",
        include_str!("../rules/unbounded-query.toml"),
    ),
    ("curl-json", include_str!("../rules/curl-json.toml")),
];

/// The scripts built-in rules name.
const BUILTIN_SCRIPTS: &[(&str, &str)] = &[
    (
        "committed-secrets.luau",
        include_str!("../rules/committed-secrets.luau"),
    ),
    (
        "push-untested.luau",
        include_str!("../rules/push-untested.luau"),
    ),
    (
        "affected-tests.luau",
        include_str!("../rules/affected-tests.luau"),
    ),
    (
        "piped-test-output.luau",
        include_str!("../rules/piped-test-output.luau"),
    ),
    (
        "pitchfork-start.luau",
        include_str!("../rules/pitchfork-start.luau"),
    ),
    (
        "pitchfork-stop.luau",
        include_str!("../rules/pitchfork-stop.luau"),
    ),
    ("job-start.luau", include_str!("../rules/job-start.luau")),
    (
        "unbounded-query.luau",
        include_str!("../rules/unbounded-query.luau"),
    ),
    (
        "scoped-lint.luau",
        include_str!("../rules/scoped-lint.luau"),
    ),
    ("checks.luau", include_str!("../rules/checks.luau")),
    (
        "claimed-path.luau",
        include_str!("../rules/claimed-path.luau"),
    ),
    (
        "reset-to-remote.luau",
        include_str!("../rules/reset-to-remote.luau"),
    ),
    (
        "rebase-merged-base.luau",
        include_str!("../rules/rebase-merged-base.luau"),
    ),
];

/// The jouet version that stops loading whole-file `.luau` rules. A test
/// fails once jouet reaches it, as a reminder to remove them.
pub const LUAU_RULES_REMOVED_IN: &str = "0.1.0";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    /// Before a tool runs: a rule may deny it.
    PreToolUse,
    /// After a tool ran, with its exit code: a rule may record a check.
    PostToolUse,
}

impl Event {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pre-tool-use" => Some(Self::PreToolUse),
            "post-tool-use" => Some(Self::PostToolUse),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PreToolUse => "pre-tool-use",
            Self::PostToolUse => "post-tool-use",
        }
    }
}

/// Where a rule came from. A user or repository rule with the name of a
/// built-in one replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    User(PathBuf),
    Repo(PathBuf),
}

impl Origin {
    pub fn scope(&self) -> Scope {
        match self {
            Self::Builtin => Scope::Builtin,
            Self::User(_) => Scope::User,
            Self::Repo(_) => Scope::Repo,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Builtin => "builtin".into(),
            Self::User(path) | Self::Repo(path) => path.display().to_string(),
        }
    }
}

/// How a rule file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// `[[rule]]` tables, any number to a file.
    Toml,
    /// One rule as a whole Luau file with a `--- why:` header: deprecated.
    Luau,
}

/// A rule file's source, before it is loaded.
#[derive(Debug, Clone)]
pub struct Source {
    /// The file's name without its extension: a Luau rule's name.
    pub name: String,
    pub origin: Origin,
    pub code: String,
    /// False for a repository rule whose directory is not trusted: it is
    /// listed, never run.
    pub trusted: bool,
    pub format: Format,
    /// Scripts the file's rules name, by the path they give, when they are
    /// not to be read from beside the file: the built-in rules', and a
    /// draft's.
    pub scripts: Vec<(String, String)>,
}

impl Source {
    /// A TOML rule file.
    pub fn toml(name: impl Into<String>, origin: Origin, code: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            origin,
            code: code.into(),
            trusted: true,
            format: Format::Toml,
            scripts: Vec::new(),
        }
    }

    /// A whole-file Luau rule.
    pub fn luau(name: impl Into<String>, origin: Origin, code: impl Into<String>) -> Self {
        Self {
            format: Format::Luau,
            ..Self::toml(name, origin, code)
        }
    }

    /// With a script its rules name, held in memory rather than read from
    /// beside the file.
    pub fn with_script(mut self, path: impl Into<String>, code: impl Into<String>) -> Self {
        self.scripts.push((path.into(), code.into()));
        self
    }

    /// Marked untrusted: listed, never run.
    pub fn untrusted(mut self) -> Self {
        self.trusted = false;
        self
    }
}

/// Whether a rule runs, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Enabled,
    /// Turned off in the settings.
    Disabled,
    /// Names a jouet command this binary does not have.
    Needs(String),
    /// A repository rule in a directory that is not trusted.
    Untrusted,
    /// Failed to compile or load.
    Broken(String),
}

pub struct Rule {
    pub name: String,
    pub origin: Origin,
    /// From the header: what the rule is for.
    pub why: String,
    pub mode: Mode,
    pub scope: Scope,
    pub author: String,
    pub created: String,
    pub event: Event,
    pub needs: Vec<String>,
    pub state: State,
    /// Its examples, as far as they could be read.
    pub examples: Vec<Case>,
    /// Why its examples do not cover both kinds, or could not be read, such
    /// as `it has no examples`. A rule of yours or a repository's with a
    /// reason here only warns, and cannot be promoted or shared.
    pub missing_examples: Option<String>,
    pub format: Format,
    /// The line its `[[rule]]` table starts on, in a TOML file.
    pub line: usize,
    /// The script a TOML rule names, as it names it.
    pub script: Option<String>,
    /// The project files a rule is about, one of which must be there for it
    /// to run.
    requires: Option<decl::Requires>,
    /// Whether the session-start primer names it, when it denies.
    pub primer: bool,
    body: Body,
}

/// What decides a rule's verdict.
enum Body {
    /// Nothing runs: the rule is broken or untrusted.
    None,
    /// A matcher: the rule fires with its message on a call it matches.
    Match {
        matchers: Vec<decl::Matcher>,
        message: String,
        suggest: Option<String>,
    },
    /// A TOML rule's script, compiled the first time it runs, behind its
    /// matchers when it has any.
    Script {
        matchers: Vec<decl::Matcher>,
        code: String,
        table: OnceCell<Result<Table, String>>,
    },
    /// A whole-file Luau rule's table.
    Luau(Table),
}

impl Rule {
    /// The command a matcher rule names to run instead.
    pub fn suggest(&self) -> Option<&str> {
        match &self.body {
            Body::Match { suggest, .. } => suggest.as_deref(),
            _ => None,
        }
    }

    /// Why this rule's file format is deprecated and how to move off it,
    /// for a rule of yours or a repository's written as a whole `.luau` file.
    pub fn deprecation(&self) -> Option<String> {
        if self.format != Format::Luau || self.scope == Scope::Builtin {
            return None;
        }
        let file = self.origin.describe();
        Some(format!(
            "{file} is a Luau rule file, which jouet stops loading in {LUAU_RULES_REMOVED_IN}: write the rule as a `[[rule]]` table in {name}.toml (`jouet rule schema` describes it), with `script = \"{name}.luau\"` only if a matcher cannot express it",
            name = self.name
        ))
    }
}

/// A tool call, as a rule sees it.
#[derive(Debug, Clone, Default)]
pub struct Call {
    pub event: Option<Event>,
    pub harness: String,
    pub tool: String,
    pub command: Option<String>,
    /// The file a file tool reads or writes.
    pub path: Option<String>,
    pub cwd: Option<String>,
    /// The agent session making the call, from the payload's `session_id`.
    pub session: Option<String>,
    /// The command's exit code, for post-tool-use.
    pub exit_code: Option<i32>,
    /// The tool's input, as the harness sent it.
    pub input: Option<serde_json::Value>,
    /// Whether the session was already shown everything the call reads,
    /// unchanged; worked out by the caller, which knows the session.
    pub seen: Option<bool>,
    /// How many lines of files the call reads; worked out by the caller.
    pub read_lines: Option<u32>,
    /// How many of them are in source files `jouet read --outline` can
    /// outline; worked out by the caller.
    pub code_lines: Option<u32>,
    /// The project's fnox config, when the call runs in a project that uses
    /// fnox; worked out by the caller.
    pub fnox: Option<Fnox>,
}

/// What a project's fnox config defines, for the matchers that ask: names
/// and paths, never values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fnox {
    /// Every secret name the config defines, in any profile.
    pub secrets: Vec<String>,
    /// The plaintext files a `plain` provider reads secrets from.
    pub files: Vec<String>,
}

impl Call {
    /// The call in a Claude Code or Codex hook payload: `hook_event_name`,
    /// `tool_name`, `tool_input.command` (or the path in its `file_path`,
    /// `notebook_path` or `path`) and `cwd`. A `PostToolUseFailure`'s
    /// `error` starts `Exit code N`; a plain `PostToolUse` succeeded. An
    /// `exit_code` field, as in a test payload, wins over both.
    pub fn from_hook(payload: &serde_json::Value, harness: &str) -> Self {
        let text = |v: Option<&serde_json::Value>| v.and_then(|v| v.as_str()).map(str::to_string);
        let hook_event = payload.get("hook_event_name").and_then(|v| v.as_str());
        let failure_code = payload
            .get("error")
            .and_then(|e| e.as_str())
            .and_then(|e| e.lines().next())
            .and_then(|line| line.strip_prefix("Exit code "))
            .and_then(|code| code.trim().parse().ok());
        let explicit = payload
            .get("exit_code")
            .and_then(|c| c.as_i64())
            .and_then(|c| i32::try_from(c).ok());
        let (event, exit_code) = match hook_event {
            Some("PostToolUse") => (Some(Event::PostToolUse), explicit.or(Some(0))),
            Some("PostToolUseFailure") => (Some(Event::PostToolUse), explicit.or(failure_code)),
            Some("PreToolUse") => (Some(Event::PreToolUse), explicit),
            _ => (None, explicit),
        };
        Self {
            event,
            harness: harness.into(),
            tool: text(payload.get("tool_name")).unwrap_or_default(),
            command: text(payload.get("tool_input").and_then(|i| i.get("command"))),
            path: ["file_path", "notebook_path", "path"]
                .iter()
                .find_map(|key| text(payload.get("tool_input").and_then(|i| i.get(*key)))),
            cwd: text(payload.get("cwd")),
            session: text(payload.get("session_id")),
            exit_code,
            input: payload.get("tool_input").cloned(),
            seen: None,
            read_lines: None,
            code_lines: None,
            fnox: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Deny,
    Warn,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub rule: String,
    pub action: Action,
    /// What the agent is told, the suggested command included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The command a matcher rule suggests running instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggest: Option<String>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Test,
    Lint,
    Build,
}

impl Kind {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "test" => Some(Self::Test),
            "lint" => Some(Self::Lint),
            "build" => Some(Self::Build),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Lint => "lint",
            Self::Build => "build",
        }
    }
}

/// A check a post-tool-use rule recorded: a test, lint or build run, how
/// much of the project it covered, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    pub rule: String,
    pub kind: Kind,
    pub command: String,
    pub exit_code: i32,
    /// Whether the run covered the whole project. A rule that does not say
    /// records a partial run.
    pub full: bool,
    /// What a partial run selected, such as `-p jouet-edit` or a test name,
    /// when the rule said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

impl Record {
    /// `full`, or `partial` and what the run selected.
    pub fn scope(&self) -> String {
        match (&self.selector, self.full) {
            (_, true) => "full".into(),
            (Some(selector), false) => format!("partial {selector}"),
            (None, false) => "partial".into(),
        }
    }
}

/// What the enabled rules made of one call.
#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    /// The first rule that denied the call; later rules do not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Decision>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Decision>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<Record>,
    /// Rules that failed, and so said nothing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Limits on each rule, per call to its `check` (and to its file when it is
/// loaded).
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// The rule's own time: its Luau, not the probes it waits on.
    pub time: Duration,
    /// The probes' time, in all: a `git` or GitHub answer the rule waits
    /// for. Starting one git process can take tens of milliseconds on a busy
    /// machine, and this keeps that from running the rule out of `time`.
    pub probes: Duration,
    /// Luau has no instruction counter; this counts its interrupts, which it
    /// raises at every function call and loop iteration.
    pub interrupts: u32,
    /// Bytes for the whole VM.
    pub memory: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            time: Duration::from_millis(50),
            probes: Duration::from_millis(200),
            interrupts: 1_000_000,
            memory: 32 * 1024 * 1024,
        }
    }
}

pub(crate) type HasCommand = dyn Fn(&[&str]) -> bool;

pub struct Options {
    /// Whether this jouet has the command at a path such as `["wait", "ci"]`.
    pub has_command: Box<HasCommand>,
    /// Rule names turned off in the settings (`"all"` for every one).
    pub disabled: Vec<String>,
    pub budget: Budget,
    /// Where compiled rules are cached, if anywhere.
    pub cache_dir: Option<PathBuf>,
}

/// State the VM's callbacks share with the engine.
pub(crate) struct Shared {
    /// What the running rule recorded; the engine names the rule.
    records: RefCell<Vec<Record>>,
    /// The rule running now, and the missing `jouet` fields rules have read.
    current: RefCell<String>,
    gaps: RefCell<Vec<Gap>>,
    deadline: Cell<Option<Instant>>,
    interrupts: Cell<u32>,
    /// The probes' time spent in the current budget.
    probe_time: Cell<Duration>,
    budget: Budget,
    /// What the probes read; None until a caller sets them up.
    probes: RefCell<Option<Probes>>,
    /// Probe answers for the current hook call.
    cache: RefCell<HashMap<String, Result<Option<serde_json::Value>, String>>>,
}

impl Shared {
    /// What is left of the current budget's time for the rule itself.
    fn remaining(&self) -> Duration {
        self.deadline.get().map_or(self.budget.time, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        })
    }

    /// What is left of the probes' time.
    fn probe_time_left(&self) -> Duration {
        self.budget.probes.saturating_sub(self.probe_time.get())
    }

    /// Counts `spent` waiting on a probe against the probes' time, and gives
    /// it back to the rule, whose clock ran meanwhile.
    fn spent_on_probe(&self, spent: Duration) {
        self.probe_time.set(self.probe_time.get() + spent);
        if let Some(deadline) = self.deadline.get() {
            self.deadline.set(Some(deadline + spent));
        }
    }

    /// Starts the budget afresh, as at the start of a rule's `check`.
    fn restart_budget(&self) {
        self.interrupts.set(0);
        self.probe_time.set(Duration::ZERO);
        self.deadline.set(Some(Instant::now() + self.budget.time));
    }
}

/// A sandboxed Luau VM with the `jouet` module, and the state its
/// callbacks share.
pub(crate) struct Vm {
    lua: Lua,
    shared: Rc<Shared>,
}

impl Vm {
    fn new(budget: Budget, has_command: Rc<HasCommand>) -> mlua::Result<Self> {
        let lua = Lua::new();
        lua.set_memory_limit(budget.memory)?;
        let shared = Rc::new(Shared {
            records: RefCell::new(Vec::new()),
            current: RefCell::new(String::new()),
            gaps: RefCell::new(Vec::new()),
            deadline: Cell::new(None),
            interrupts: Cell::new(0),
            probe_time: Cell::new(Duration::ZERO),
            budget,
            probes: RefCell::new(None),
            cache: RefCell::new(HashMap::new()),
        });
        install_module(&lua, &shared, &has_command)?;
        let guard = shared.clone();
        lua.set_interrupt(move |_| {
            let Some(deadline) = guard.deadline.get() else {
                return Ok(VmState::Continue);
            };
            let count = guard.interrupts.get() + 1;
            guard.interrupts.set(count);
            if count > guard.budget.interrupts {
                return Err(mlua::Error::runtime("exceeded its instruction budget"));
            }
            if Instant::now() > deadline {
                return Err(mlua::Error::runtime("exceeded its time budget"));
            }
            Ok(VmState::Continue)
        });
        lua.sandbox(true)?;
        Ok(Self { lua, shared })
    }

    /// Compiles (or reads from the cache) and runs a script file, in an
    /// environment of its own, returning the table it returns.
    fn load_table(
        &self,
        name: &str,
        code: &str,
        cache_dir: Option<&Path>,
    ) -> Result<Table, String> {
        self.shared.current.replace(name.to_string());
        let (bytecode, cached) = cache::compile(code, cache_dir)?;
        let value = match self.run_chunk(name, &bytecode) {
            // A cached chunk from another Luau version, or a damaged file:
            // compile it again rather than leave the script broken.
            Err(_) if cached => self.run_chunk(name, &cache::recompile(code, cache_dir)?)?,
            result => result?,
        };
        match value {
            Value::Table(table) => Ok(table),
            other => Err(format!(
                "the file must return a table, not {}",
                other.type_name()
            )),
        }
    }

    /// Runs a compiled file in an environment of its own.
    fn run_chunk(&self, name: &str, bytecode: &[u8]) -> Result<Value, String> {
        let env = self.lua.create_table().map_err(|e| e.to_string())?;
        let meta = self.lua.create_table().map_err(|e| e.to_string())?;
        meta.set("__index", self.lua.globals())
            .map_err(|e| e.to_string())?;
        env.set_metatable(Some(meta)).map_err(|e| e.to_string())?;
        self.guarded(|| {
            self.lua
                .load(bytecode)
                .set_name(format!("={name}"))
                .set_mode(mlua::chunk::ChunkMode::Binary)
                .set_environment(env)
                .eval()
        })
    }

    /// Runs `f` under the budget.
    fn guarded<T>(&self, f: impl FnOnce() -> mlua::Result<T>) -> Result<T, String> {
        self.shared.restart_budget();
        let result = f();
        self.shared.deadline.set(None);
        result.map_err(|e| first_line(&e.to_string()))
    }

    /// Forgets the probes' answers: a new hook call asks again.
    fn new_call(&self) {
        self.shared.cache.borrow_mut().clear();
    }
}

pub struct Engine {
    /// Started the first time a script runs, or a Luau rule file loads.
    vm: OnceCell<Result<Vm, String>>,
    budget: Budget,
    has_command: Rc<HasCommand>,
    cache_dir: Option<PathBuf>,
    /// What the probes read, kept for a VM that has not started yet.
    probes: RefCell<Option<Probes>>,
    rules: Vec<Rule>,
}

/// The built-in rules' sources.
pub fn builtin() -> Vec<Source> {
    BUILTIN
        .iter()
        .map(|(name, code)| {
            BUILTIN_SCRIPTS.iter().fold(
                Source::toml(*name, Origin::Builtin, *code),
                |source, (path, script)| source.with_script(*path, *script),
            )
        })
        .collect()
}

/// The rule files in a directory, `*.toml` and `*.luau`, sorted by name.
/// Some of the `.luau` files may be scripts a TOML rule names rather than
/// rules; the engine tells them apart.
pub fn rule_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| format_of(p).is_some() && p.is_file())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    files.sort();
    Ok(files)
}

/// A rule file's format, from its extension.
fn format_of(path: &Path) -> Option<Format> {
    match path.extension()?.to_str()? {
        "toml" => Some(Format::Toml),
        "luau" => Some(Format::Luau),
        _ => None,
    }
}

/// Every rule source: built-in, then the user's, then a repository's
/// (marked untrusted unless its directory is trusted). Unreadable files are
/// reported, not fatal.
pub fn discover(
    user_dir: Option<&Path>,
    repo_dir: Option<&Path>,
    trust: &TrustStore,
) -> (Vec<Source>, Vec<String>) {
    discover_after(builtin(), user_dir, repo_dir, trust)
}

/// `sources`, then the rule files of the user's directory, then a
/// repository's (untrusted unless its directory is trusted).
fn discover_after(
    mut sources: Vec<Source>,
    user_dir: Option<&Path>,
    repo_dir: Option<&Path>,
    trust: &TrustStore,
) -> (Vec<Source>, Vec<String>) {
    let mut errors = Vec::new();
    let mut add = |dir: &Path, repo: bool, trusted: bool, sources: &mut Vec<Source>| {
        let files = match rule_files(dir) {
            Ok(files) => files,
            Err(e) => {
                errors.push(format!("reading {}: {e}", dir.display()));
                return;
            }
        };
        for path in files {
            let format = format_of(&path).unwrap_or(Format::Toml);
            let code = match std::fs::read_to_string(&path) {
                Ok(code) => code,
                Err(e) => {
                    errors.push(format!("reading {}: {e}", path.display()));
                    continue;
                }
            };
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let origin = if repo {
                Origin::Repo(path)
            } else {
                Origin::User(path)
            };
            let source = Source {
                trusted,
                format,
                ..Source::toml(name, origin, code)
            };
            sources.push(source);
        }
    };
    if let Some(dir) = user_dir {
        add(dir, false, true, &mut sources);
    }
    if let Some(dir) = repo_dir {
        let trusted = trust.check(dir) == Trust::Trusted;
        add(dir, true, trusted, &mut sources);
    }
    (sources, errors)
}

impl Engine {
    /// Loads every rule. TOML rules load without a VM; a Luau rule file
    /// starts one. A rule that fails to load is kept, as [`State::Broken`],
    /// so it can be listed. A `.luau` file a TOML rule beside it names as
    /// its `script`, or one beside a TOML file of the same name, is a
    /// script, not a rule of its own.
    pub fn new(sources: Vec<Source>, options: Options) -> Self {
        let Options {
            has_command,
            disabled,
            budget,
            cache_dir,
        } = options;
        let mut engine = Self {
            vm: OnceCell::new(),
            budget,
            has_command: Rc::from(has_command),
            cache_dir,
            probes: RefCell::new(None),
            rules: Vec::new(),
        };
        let (toml, luau): (Vec<_>, Vec<_>) = sources
            .into_iter()
            .enumerate()
            .partition(|(_, s)| s.format == Format::Toml);
        // Even a TOML file that does not load claims its namesake script.
        let mut scripts: Vec<PathBuf> = toml
            .iter()
            .filter_map(|(_, s)| match &s.origin {
                Origin::User(path) | Origin::Repo(path) => Some(path.with_extension("luau")),
                Origin::Builtin => None,
            })
            .collect();
        let mut loaded = Vec::new();
        for (order, source) in toml {
            let rules = engine.load_toml(source, &disabled, &mut scripts);
            loaded.extend(rules.into_iter().map(|rule| (order, rule)));
        }
        for (order, source) in luau {
            let is_script = match &source.origin {
                Origin::User(path) | Origin::Repo(path) => scripts.contains(path),
                Origin::Builtin => false,
            };
            if !is_script {
                loaded.push((order, engine.load_luau(source, &disabled)));
            }
        }
        // Rules run in the order their sources came, whatever their format.
        loaded.sort_by_key(|(order, _)| *order);
        for (_, mut rule) in loaded {
            // The floor: a rule of yours or a repository's adds decisions,
            // and never takes the place of a built-in one.
            if !matches!(rule.state, State::Untrusted | State::Broken(_))
                && let Some(other) = engine
                    .rules
                    .iter()
                    .find(|r| r.name == rule.name && r.state != State::Untrusted)
            {
                rule.state = State::Broken(match other.scope {
                    Scope::Builtin => format!(
                        "`{}` is a built-in rule, which cannot be replaced; rename it",
                        rule.name
                    ),
                    _ => format!(
                        "{} already defines `{}`",
                        other.origin.describe(),
                        rule.name
                    ),
                });
                rule.body = Body::None;
            }
            engine.rules.push(rule);
        }
        engine
    }

    /// The VM, started on first use with the probes set so far.
    fn vm(&self) -> Result<&Vm, String> {
        self.vm
            .get_or_init(|| {
                let vm = Vm::new(self.budget, self.has_command.clone())
                    .map_err(|e| format!("starting the Luau VM: {e}"))?;
                if let Some(probes) = self.probes.borrow().clone() {
                    *vm.shared.probes.borrow_mut() = Some(probes);
                }
                Ok(vm)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The rules in a TOML file. Adds the paths of the scripts they name to
    /// `scripts`.
    fn load_toml(
        &self,
        source: Source,
        disabled: &[String],
        scripts: &mut Vec<PathBuf>,
    ) -> Vec<Rule> {
        let scope = source.origin.scope();
        let file = source.origin.describe();
        let placeholder = |state: State| Rule {
            name: source.name.clone(),
            origin: source.origin.clone(),
            why: String::new(),
            mode: scope.default_mode(),
            scope,
            author: String::new(),
            created: String::new(),
            event: Event::PreToolUse,
            needs: Vec::new(),
            state,
            examples: Vec::new(),
            missing_examples: None,
            format: Format::Toml,
            line: 1,
            script: None,
            requires: None,
            primer: true,
            body: Body::None,
        };
        let decls = match decl::parse(&source.code) {
            Ok(decls) => decls,
            Err((line, e)) => {
                return vec![placeholder(State::Broken(format!("{file}:{line}: {e}")))];
            }
        };
        if decls.is_empty() {
            return vec![placeholder(State::Broken(format!(
                "{file}: it has no `[[rule]]` tables"
            )))];
        }
        let dir = match &source.origin {
            Origin::User(path) | Origin::Repo(path) => path.parent().map(Path::to_path_buf),
            Origin::Builtin => None,
        };
        decls
            .into_iter()
            .map(|(line, d)| {
                if let (Some(dir), Some(script)) = (&dir, &d.script) {
                    scripts.push(dir.join(&script.0));
                }
                let mut rule = self.toml_rule(&source, line, d, dir.as_deref(), disabled);
                if let State::Broken(e) = &mut rule.state {
                    *e = format!("{file}:{line}: {e}");
                }
                rule
            })
            .collect()
    }

    fn toml_rule(
        &self,
        source: &Source,
        line: usize,
        d: decl::RuleDecl,
        dir: Option<&Path>,
        disabled: &[String],
    ) -> Rule {
        let scope = source.origin.scope();
        let examples = examples::from_decl(d.examples.as_ref());
        let event = d.on.unwrap_or(Event::PreToolUse);
        let problem = if d.scope.is_some_and(|s| s != scope) {
            Some(format!(
                "it says scope `{}`, but this is a {} rule",
                d.scope.map_or("", Scope::name),
                scope.name()
            ))
        } else if d.why.trim().is_empty() {
            Some("`why` says what the rule is for, in one line".into())
        } else {
            rule_problem(&d, event)
        };
        let mut rule = Rule {
            name: d.name.0,
            origin: source.origin.clone(),
            why: d.why,
            mode: d.mode.unwrap_or(scope.default_mode()),
            scope,
            author: d.author.unwrap_or_else(|| match scope {
                Scope::Builtin => "builtin".into(),
                _ => "unknown".into(),
            }),
            created: d.created.unwrap_or_default(),
            event,
            needs: d.needs,
            state: State::Enabled,
            examples: Vec::new(),
            missing_examples: None,
            format: Format::Toml,
            line,
            script: d.script.as_ref().map(|s| s.0.clone()),
            requires: d.requires,
            primer: d.primer.unwrap_or(true),
            body: Body::None,
        };
        let problem = problem.or_else(|| examples.as_ref().err().cloned());
        if let Some(problem) = problem {
            rule.state = State::Broken(problem);
            return rule;
        }
        if !source.trusted {
            rule.state = State::Untrusted;
            return rule;
        }
        (rule.examples, rule.missing_examples) = examples.unwrap_or_default();
        rule.body = match (d.script, d.message) {
            (Some(script), _) => {
                let code = match source.scripts.iter().find(|(path, _)| *path == script.0) {
                    Some((_, code)) => Ok(code.clone()),
                    None => match dir {
                        Some(dir) => std::fs::read_to_string(dir.join(&script.0))
                            .map_err(|e| format!("reading its script {}: {e}", script.0)),
                        None => Err(format!("its script {} is not built in", script.0)),
                    },
                };
                match code {
                    Ok(code) => Body::Script {
                        matchers: d.matchers,
                        code,
                        table: OnceCell::new(),
                    },
                    Err(e) => {
                        rule.state = State::Broken(e);
                        return rule;
                    }
                }
            }
            (None, message) => Body::Match {
                matchers: d.matchers,
                message: compose(message.as_deref().unwrap_or_default(), d.suggest.as_deref()),
                suggest: d.suggest,
            },
        };
        self.settle(&mut rule, disabled);
        rule
    }

    /// The last steps of loading a rule that runs: the warn-only gate for
    /// a rule without examples of both kinds, then whether it is disabled or
    /// needs a command jouet lacks.
    fn settle(&self, rule: &mut Rule, disabled: &[String]) {
        // A rule of yours or a repository's proves itself on examples of
        // both kinds before it may deny anything.
        if rule.scope != Scope::Builtin && rule.missing_examples.is_some() {
            rule.mode = Mode::Warn;
        }
        let needs: Vec<&str> = rule.needs.iter().map(String::as_str).collect();
        if disabled.iter().any(|d| d == &rule.name || d == "all") {
            rule.state = State::Disabled;
        } else if !needs.is_empty() && !(self.has_command)(&needs) {
            rule.state = State::Needs(format!("jouet {}", needs.join(" ")));
        }
    }

    /// A whole-file Luau rule: its header, then its table, which starts the
    /// VM.
    fn load_luau(&self, source: Source, disabled: &[String]) -> Rule {
        let scope = source.origin.scope();
        let header = header::parse(&source.code);
        let mut rule = Rule {
            name: source.name,
            origin: source.origin,
            why: header.why.clone().unwrap_or_default(),
            mode: scope.default_mode(),
            scope,
            author: header.author.clone().unwrap_or_else(|| match scope {
                Scope::Builtin => "builtin".into(),
                _ => "unknown".into(),
            }),
            created: header.created.clone().unwrap_or_default(),
            event: Event::PreToolUse,
            needs: Vec::new(),
            state: State::Enabled,
            examples: Vec::new(),
            missing_examples: None,
            format: Format::Luau,
            line: 1,
            script: None,
            requires: None,
            primer: true,
            body: Body::None,
        };
        if let Err(e) = apply_header(&mut rule, &header) {
            rule.state = State::Broken(e);
            return rule;
        }
        if !source.trusted {
            rule.state = State::Untrusted;
            return rule;
        }
        let table = match self
            .vm()
            .and_then(|vm| vm.load_table(&rule.name, &source.code, self.cache_dir.as_deref()))
        {
            Ok(table) => table,
            Err(e) => {
                rule.state = State::Broken(e);
                return rule;
            }
        };
        if let Err(e) = read_fields(&mut rule, &table) {
            rule.state = State::Broken(e);
            return rule;
        }
        (rule.examples, rule.missing_examples) = examples::read(&table);
        rule.body = Body::Luau(table);
        self.settle(&mut rule, disabled);
        rule
    }

    /// Compiles every script that has not run yet, so a script that does not
    /// compile shows as broken. Listing and testing do this; a hook leaves a
    /// script until a call needs it.
    pub fn load_scripts(&mut self) {
        let mut broken = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            if let Body::Script { code, table, .. } = &rule.body
                && rule.state != State::Untrusted
                && let Err(e) = self.script(&rule.name, code, table)
            {
                broken.push((i, e));
            }
        }
        for (i, e) in broken {
            let rule = &mut self.rules[i];
            rule.state = State::Broken(format!(
                "{}:{}: {}: {e}",
                rule.origin.describe(),
                rule.line,
                rule.script.as_deref().unwrap_or("")
            ));
            rule.body = Body::None;
        }
    }

    /// A script's table, compiling and running the file the first time.
    fn script<'a>(
        &self,
        name: &str,
        code: &str,
        table: &'a OnceCell<Result<Table, String>>,
    ) -> Result<&'a Table, String> {
        table
            .get_or_init(|| {
                let vm = self.vm()?;
                let table = vm.load_table(name, code, self.cache_dir.as_deref())?;
                let check: Value = table.raw_get("check").map_err(|e| e.to_string())?;
                if !check.is_function() {
                    return Err("the script must return a table with a `check` function".into());
                }
                for key in ["examples", "on", "needs"] {
                    let value: Value = table.raw_get(key).map_err(|e| e.to_string())?;
                    if !value.is_nil() {
                        return Err(format!(
                            "the script sets `{key}`, which belongs in the rule's TOML"
                        ));
                    }
                }
                Ok(table)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Lets the rules' probes run, reading what `probes` points at. Without
    /// this every probe returns nil and says why.
    pub fn set_probes(&self, probes: Probes) {
        if let Some(Ok(vm)) = self.vm.get() {
            *vm.shared.probes.borrow_mut() = Some(probes.clone());
            vm.new_call();
        }
        *self.probes.borrow_mut() = Some(probes);
    }

    /// Whether a Luau VM has started: only scripts and Luau rule files start
    /// one.
    pub fn vm_started(&self) -> bool {
        self.vm.get().is_some()
    }

    /// The primitives rules reached for that the `jouet` module does not
    /// have, since the last call: what the API could not express.
    pub fn take_gaps(&self) -> Vec<Gap> {
        let Some(Ok(vm)) = self.vm.get() else {
            return Vec::new();
        };
        let mut gaps = vm.shared.gaps.take();
        gaps.sort();
        gaps.dedup();
        gaps
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    /// Forgets the probes' answers, when a VM has started: a new hook call
    /// asks again.
    fn new_call(&self) {
        if let Some(Ok(vm)) = self.vm.get() {
            vm.new_call();
        }
    }

    /// Runs every enabled rule for the call's event, in order, stopping at
    /// the first denial.
    pub fn evaluate(&self, call: &Call) -> Outcome {
        self.new_call();
        let commands = call_commands(call);
        let mut outcome = Outcome::default();
        let event = call.event.unwrap_or(Event::PreToolUse);
        for rule in &self.rules {
            if rule.state != State::Enabled || rule.event != event {
                continue;
            }
            match self.run_check(rule, call, &commands, true) {
                Ok((decision, records)) => {
                    outcome.records.extend(records);
                    match decision {
                        Some(d) if d.action == Action::Deny => {
                            outcome.deny = Some(d);
                            break;
                        }
                        Some(d) if d.action == Action::Warn => outcome.warnings.push(d),
                        _ => {}
                    }
                }
                Err(e) => outcome.errors.push(format!("rule {}: {e}", rule.name)),
            }
        }
        outcome
    }

    /// Runs one rule, whatever its state, and returns its decision, as
    /// enforced (a warn-mode rule's denial is a warning), and anything it
    /// recorded. A call of its own: probes are asked afresh.
    pub fn check(
        &self,
        rule: &Rule,
        call: &Call,
    ) -> Result<(Option<Decision>, Vec<Record>), String> {
        self.new_call();
        self.run_check(rule, call, &call_commands(call), true)
    }

    /// `check`, with `enforce` deciding whether the rule's mode and the floor
    /// apply to its decision. Examples describe what a rule decides, so they
    /// are run without them: a warn-mode rule that denies fires as `deny`.
    fn run_check(
        &self,
        rule: &Rule,
        call: &Call,
        commands: &[shell::Command],
        enforce: bool,
    ) -> Result<(Option<Decision>, Vec<Record>), String> {
        let not_loaded = || match &rule.state {
            State::Broken(e) => e.clone(),
            _ => "the rule is not loaded".into(),
        };
        let mut suggest = None;
        let (returned, records) = match &rule.body {
            Body::None => return Err(not_loaded()),
            Body::Match {
                matchers,
                message,
                suggest: suggested,
            } => {
                if !matcher::any(matchers, call, commands) || !self.in_use(rule, call, enforce) {
                    return Ok((None, Vec::new()));
                }
                suggest.clone_from(suggested);
                (
                    Some(Ok(("deny".to_string(), Some(message.clone())))),
                    Vec::new(),
                )
            }
            Body::Script {
                matchers,
                code,
                table,
            } => {
                if !matchers.is_empty() && !matcher::any(matchers, call, commands) {
                    return Ok((None, Vec::new()));
                }
                if !self.in_use(rule, call, enforce) {
                    return Ok((None, Vec::new()));
                }
                let table = self.script(&rule.name, code, table)?;
                let matched = matcher::matched(matchers, call, commands);
                self.run_luau(rule, table, call, &matched)?
            }
            Body::Luau(table) => self.run_luau(rule, table, call, &[])?,
        };
        let decision = match returned {
            None => None,
            Some(Ok((action, message))) => {
                let action = match action.as_str() {
                    // A warn-mode rule only warns, and the floor: a rule of
                    // yours or a repository's cannot lock you out of jouet.
                    "deny"
                        if enforce
                            && (rule.mode == Mode::Warn
                                || (rule.scope != Scope::Builtin && only_jouet(call))) =>
                    {
                        Action::Warn
                    }
                    "deny" => Action::Deny,
                    "warn" => Action::Warn,
                    "allow" => Action::Allow,
                    other => return Err(format!("check returned an unknown action `{other}`")),
                };
                Some(Decision {
                    rule: rule.name.clone(),
                    action,
                    message,
                    suggest,
                })
            }
            Some(Err(type_name)) => {
                return Err(format!(
                    "check must return jouet.deny(...), jouet.warn(...), jouet.allow() or nil, not {type_name}"
                ));
            }
        };
        Ok((decision, records))
    }

    /// Whether a file a rule requires is in the call's directory or one
    /// above it. Only a call that is enforced asks: examples run as if one
    /// were.
    fn in_use(&self, rule: &Rule, call: &Call, enforce: bool) -> bool {
        !enforce || self.in_use_at(rule, call.cwd.as_deref().map(Path::new))
    }

    /// Whether a file a rule requires is in the probes' directory or one
    /// above it, as for a call made there. A rule that requires none always
    /// applies.
    pub fn in_use_here(&self, rule: &Rule) -> bool {
        self.in_use_at(rule, None)
    }

    /// Looks in `cwd`, else the probes' directory, else this process's.
    fn in_use_at(&self, rule: &Rule, cwd: Option<&Path>) -> bool {
        let Some(requires) = &rule.requires else {
            return true;
        };
        let probes = self.probes.borrow();
        let here = std::env::current_dir().ok();
        let dir = cwd
            .map(Path::to_path_buf)
            .or_else(|| probes.as_ref().and_then(Probes::dir))
            .or(here);
        dir.is_some_and(|dir| probe::has_file(&requires.files, &dir))
    }

    /// Runs a Luau rule's `check` under its budget: the action and message
    /// it returned (or the type of what it returned instead), and what it
    /// recorded.
    fn run_luau(
        &self,
        rule: &Rule,
        table: &Table,
        call: &Call,
        matched: &[bool],
    ) -> Result<(LuauVerdict, Vec<Record>), String> {
        let vm = self.vm()?;
        vm.shared.current.replace(rule.name.clone());
        // Raw reads throughout: a rule's tables can carry metamethods, which
        // must never run outside the budget.
        let check: mlua::Function = table.raw_get("check").map_err(|e| e.to_string())?;
        let call_table = call_table(&vm.lua, call, matched).map_err(|e| e.to_string())?;
        vm.shared.records.borrow_mut().clear();
        let returned = vm.guarded(|| {
            Ok(match check.call::<Value>(call_table)? {
                Value::Nil => None,
                Value::Table(t) => Some(Ok((
                    t.raw_get::<Option<String>>("action")?.unwrap_or_default(),
                    t.raw_get::<Option<String>>("message")?,
                ))),
                other => Some(Err(other.type_name())),
            })
        })?;
        let records = vm
            .shared
            .records
            .borrow_mut()
            .drain(..)
            .map(|record| Record {
                rule: rule.name.clone(),
                ..record
            })
            .collect();
        Ok((returned, records))
    }
}

/// What a Luau `check` returned: nothing, an action and message, or the
/// type of something else.
type LuauVerdict = Option<Result<(String, Option<String>), &'static str>>;

/// A matcher rule's message, with the command it suggests in front.
fn compose(message: &str, suggest: Option<&str>) -> String {
    match suggest {
        Some(command) => format!("Run `{command}` instead. {message}"),
        None => message.to_string(),
    }
}

/// What is wrong with a TOML rule's keys taken together, beyond what each
/// key's type checks.
fn rule_problem(d: &decl::RuleDecl, event: Event) -> Option<String> {
    let matchers = &d.matchers;
    let message = d.message.as_deref();
    if let Some(r) = &d.requires {
        if r.files.is_empty() {
            return Some("`requires` names the `files` the rule is about".into());
        }
        if let Some(file) = r.files.iter().find(|f| {
            f.is_empty()
                || Path::new(f)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
        }) {
            return Some(format!(
                "`requires.files` are paths within a directory, such as \"pitchfork.toml\", not `{file}`"
            ));
        }
    }
    for (i, m) in matchers.iter().enumerate() {
        if let Some(problem) = m.problem(false) {
            let at = if matchers.len() == 1 {
                "`match`".to_string()
            } else {
                format!("`match` {}", i + 1)
            };
            return Some(format!("{at}: {problem}"));
        }
    }
    match &d.script {
        Some(_) if message.is_some() || d.suggest.is_some() => Some(
            "a script's `check` returns its own message: leave out `message` and `suggest`".into(),
        ),
        Some(_) => None,
        None if matchers.is_empty() => {
            Some("give it a `match`, or a `script` for logic a matcher cannot express".into())
        }
        None if message.is_none_or(|m| m.trim().is_empty()) => {
            Some("give it a `message`: what the agent is told when it matches".into())
        }
        None if event == Event::PostToolUse => Some(
            "a matcher denies or warns before a tool runs; a `post-tool-use` rule needs a `script`"
                .into(),
        ),
        None => None,
    }
}

/// Every command in a call's shell command, nested ones included.
fn call_commands(call: &Call) -> Vec<shell::Command> {
    match &call.command {
        Some(script) => {
            let (top, nested) = commands(script);
            top.into_iter().chain(nested).collect()
        }
        None => Vec::new(),
    }
}

/// A script's top-level commands, and every command it runs besides: those
/// inside `$(...)`, then those in scripts passed to `sh -c` and the like.
fn commands(script: &str) -> (Vec<shell::Command>, Vec<shell::Command>) {
    let parsed = shell::parse(script);
    let mut nested = parsed.nested.clone();
    let mut queue = parsed.inner_scripts();
    let mut seen = 0;
    while let Some(inner) = queue.pop() {
        seen += 1;
        if seen > 16 {
            break;
        }
        nested.extend(inner.all().into_iter().cloned());
        queue.extend(inner.inner_scripts());
    }
    (parsed.commands, nested)
}

/// A rule reading a field of the `jouet` module that does not exist, such
/// as `jouet.pr_status`: a primitive it wanted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Gap {
    pub rule: String,
    pub primitive: String,
}

/// A rule's verdict in a word: `deny`, `warn`, `record test` or `allow`.
pub fn summarize(decision: Option<&Decision>, records: &[Record]) -> String {
    if let Some(record) = records.first() {
        return format!("record {}", record.kind.name());
    }
    match decision.map(|d| d.action) {
        Some(Action::Deny) => "deny".into(),
        Some(Action::Warn) => "warn".into(),
        _ => "allow".into(),
    }
}

/// An error's first line, without mlua's `runtime error: ` prefix.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text);
    line.strip_prefix("runtime error: ")
        .unwrap_or(line)
        .to_string()
}

/// Checks the header against the file: its name, its scope, and a `why`.
fn apply_header(rule: &mut Rule, header: &header::Header) -> Result<(), String> {
    if header.why.is_none() {
        return Err("start the file with a `--- why: ...` header line".into());
    }
    if let Some(name) = &header.name
        && name != &rule.name
    {
        return Err(format!(
            "the header names `{name}`, but the file is `{}.luau`",
            rule.name
        ));
    }
    if let Some(scope) = &header.scope
        && scope != rule.scope.name()
    {
        return Err(format!(
            "the header says scope `{scope}`, but this is a {} rule",
            rule.scope.name()
        ));
    }
    if let Some(mode) = &header.mode {
        rule.mode = match mode.as_str() {
            "warn" => Mode::Warn,
            "deny" => Mode::Deny,
            other => return Err(format!("`mode` must be warn or deny, not `{other}`")),
        };
    }
    Ok(())
}

/// Whether every top-level command in the call runs jouet itself.
fn only_jouet(call: &Call) -> bool {
    let Some(script) = &call.command else {
        return false;
    };
    let commands = shell::parse(script).commands;
    !commands.is_empty() && commands.iter().all(|c| c.program() == Some("jouet"))
}

/// Reads the rule table's fields with raw access, so no metamethod of the
/// rule's runs outside its budget.
fn read_fields(rule: &mut Rule, table: &Table) -> Result<(), String> {
    let check: Value = table.raw_get("check").map_err(|e| e.to_string())?;
    if !check.is_function() {
        return Err("the rule table needs a `check` function".into());
    }
    if let Some(on) = table
        .raw_get::<Option<String>>("on")
        .map_err(|e| format!("`on`: {e}"))?
    {
        rule.event = Event::parse(&on).ok_or_else(|| {
            format!("`on` must be \"pre-tool-use\" or \"post-tool-use\", not `{on}`")
        })?;
    }
    if let Some(needs) = table
        .raw_get::<Option<Table>>("needs")
        .map_err(|e| format!("`needs`: {e}"))?
    {
        rule.needs = needs
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()
            .map_err(|e| format!("`needs`: {e}"))?;
    }
    Ok(())
}

/// The `jouet` module: the Rust functions and the probes, then the
/// prelude's.
fn install_module(
    lua: &Lua,
    shared: &Rc<Shared>,
    has_command: &Rc<HasCommand>,
) -> mlua::Result<()> {
    let jouet = lua.create_table()?;
    // `jouet.git(command)` is the prelude's, through `__call`; the table
    // also holds the git probe.
    let git = lua.create_table()?;
    jouet.set("git", &git)?;
    probe::install(lua, &jouet, &git, shared)?;
    jouet.set(
        "parse",
        lua.create_function(|lua, (script, exit_code): (String, Option<i32>)| {
            commands_table(lua, &script, exit_code, &[])
        })?,
    )?;
    jouet.set(
        "unwrap",
        lua.create_function(|_, words: Vec<String>| {
            let command = shell::Command {
                words,
                ..shell::Command::default()
            };
            let via: Vec<String> = command.via().into_iter().map(String::from).collect();
            Ok((command.argv().to_vec(), via))
        })?,
    )?;
    let has = has_command.clone();
    jouet.set(
        "has_command",
        lua.create_function(move |_, path: Variadic<String>| {
            let words: Vec<&str> = path.iter().map(String::as_str).collect();
            Ok(!words.is_empty() && has(&words))
        })?,
    )?;
    let records = shared.clone();
    jouet.set(
        "record",
        lua.create_function(
            move |_,
                  (kind, command, scope, selector): (
                String,
                Table,
                Option<String>,
                Option<String>,
            )| {
                let kind = Kind::parse(&kind).ok_or_else(|| {
                    mlua::Error::runtime(format!(
                        "jouet.record: kind must be \"test\", \"lint\" or \"build\", not `{kind}`"
                    ))
                })?;
                // Unknown means partial: only a rule that knows the run
                // covered everything says so.
                let full = match scope.as_deref() {
                    None | Some("partial") => false,
                    Some("full") => true,
                    Some(other) => {
                        return Err(mlua::Error::runtime(format!(
                            "jouet.record: scope must be \"full\" or \"partial\", not `{other}`"
                        )));
                    }
                };
                // A command whose exit status is unknown says nothing.
                if let Some(status) = command.raw_get::<Option<i32>>("status")? {
                    records.records.borrow_mut().push(Record {
                        rule: String::new(),
                        kind,
                        command: command.raw_get("text")?,
                        exit_code: status,
                        full,
                        selector: selector.filter(|s| !full && !s.is_empty()),
                    });
                }
                Ok(())
            },
        )?,
    )?;
    lua.load(PRELUDE)
        .set_name("=jouet")
        .call::<()>(jouet.clone())?;
    // Reading a field the module lacks still gives nil, and records the
    // primitive the rule wanted.
    let meta = lua.create_table()?;
    let gaps = shared.clone();
    meta.set(
        "__index",
        lua.create_function(move |_, (_, key): (Value, Value)| {
            if let Value::String(key) = key {
                let mut list = gaps.gaps.borrow_mut();
                if list.len() < 64 {
                    list.push(Gap {
                        rule: gaps.current.borrow().clone(),
                        primitive: key.to_string_lossy(),
                    });
                }
            }
            Ok(Value::Nil)
        })?,
    )?;
    jouet.set_metatable(Some(meta))?;
    let globals = lua.globals();
    globals.set("jouet", jouet)?;
    // Rules read the outside world only through `jouet`.
    globals.set("require", Value::Nil)?;
    Ok(())
}

/// `matched` says, in the order of `call_commands`, which commands the
/// rule's matchers matched.
fn call_table(lua: &Lua, call: &Call, matched: &[bool]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("event", call.event.unwrap_or(Event::PreToolUse).name())?;
    t.set("harness", call.harness.as_str())?;
    t.set("tool", call.tool.as_str())?;
    t.set("command", call.command.as_deref())?;
    t.set("path", call.path.as_deref())?;
    t.set("cwd", call.cwd.as_deref())?;
    t.set("session", call.session.as_deref())?;
    t.set("exit_code", call.exit_code)?;
    let commands = match &call.command {
        Some(script) => commands_table(lua, script, call.exit_code, matched)?,
        None => lua.create_table()?,
    };
    t.set("commands", commands)?;
    Ok(t)
}

/// Every command a script runs, as tables: the top-level ones with their
/// exit status when it is known, then those inside `$(...)`, then those in
/// scripts passed to `sh -c` and the like.
fn commands_table(
    lua: &Lua,
    script: &str,
    exit_code: Option<i32>,
    matched: &[bool],
) -> mlua::Result<Table> {
    let (top, nested) = commands(script);
    let statuses = match exit_code {
        Some(code) => shell::statuses(&top, code),
        None => vec![None; top.len()],
    };
    let hit = |i: usize| matched.get(i).copied().unwrap_or(false);
    let list = lua.create_table()?;
    for (i, (command, status)) in top.iter().zip(statuses).enumerate() {
        list.push(command_table(lua, command, false, status, hit(i))?)?;
    }
    for (i, command) in nested.iter().enumerate() {
        list.push(command_table(lua, command, true, None, hit(top.len() + i))?)?;
    }
    Ok(list)
}

fn command_table(
    lua: &Lua,
    command: &shell::Command,
    nested: bool,
    status: Option<i32>,
    matched: bool,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let argv = command.argv();
    t.set("words", command.words.clone())?;
    t.set("argv", argv.to_vec())?;
    t.set("program", command.program())?;
    t.set("heredocs", command.heredocs.clone())?;
    t.set("via", command.via())?;
    t.set(
        "op",
        command.op.map(|op| match op {
            shell::Op::Seq => "seq",
            shell::Op::And => "and",
            shell::Op::Or => "or",
            shell::Op::Pipe => "pipe",
            shell::Op::Background => "background",
        }),
    )?;
    t.set("stdout_to", command.stdout_to.as_deref())?;
    t.set("env", command.env.clone())?;
    t.set("nested", nested)?;
    t.set("status", status)?;
    t.set("matched", matched)?;
    let text = argv.join(" ");
    let text = match text.char_indices().nth(80) {
        Some((i, _)) => format!("{}...", &text[..i]),
        None => text,
    };
    t.set("text", text)?;
    Ok(t)
}

#[cfg(test)]
mod tests;
