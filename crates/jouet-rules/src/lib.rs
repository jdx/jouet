//! jouet's rule engine: small Luau scripts that read the command an agent is
//! about to run, or has just run, and decide what to do about it.
//!
//! A rule is a `.luau` file returning a table with a `check` function. It runs
//! in a sandboxed Luau VM whose only way out is the `jouet` module: the tool
//! call it is given, a shell-aware view of its commands, helpers for matching
//! them, and the actions `deny`, `warn`, `allow` and `record`. The built-in
//! rules are written against the same API as anyone else's.
//!
//! Every failure is contained: a rule that does not compile, throws, runs past
//! its time or instruction budget or returns something unexpected is reported
//! in [`Outcome::errors`] and treated as having said nothing.

mod cache;
pub mod shell;
mod trust;

pub use trust::{Trust, TrustStore};

use mlua::{Lua, Table, Value, Variadic, VmState};
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The Luau type definitions of the `jouet` module, for type-checking rules.
pub const TYPES: &str = include_str!("../jouet.d.luau");

const PRELUDE: &str = include_str!("prelude.luau");

/// The built-in rules, embedded in the binary.
const BUILTIN: &[(&str, &str)] = &[
    ("sleep-poll", include_str!("../rules/sleep-poll.luau")),
    (
        "review-threads",
        include_str!("../rules/review-threads.luau"),
    ),
    ("python-edit", include_str!("../rules/python-edit.luau")),
    ("git-add-all", include_str!("../rules/git-add-all.luau")),
    ("git-stash", include_str!("../rules/git-stash.luau")),
    ("no-verify", include_str!("../rules/no-verify.luau")),
    ("force-push", include_str!("../rules/force-push.luau")),
    ("checks", include_str!("../rules/checks.luau")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    pub fn describe(&self) -> String {
        match self {
            Self::Builtin => "builtin".into(),
            Self::User(path) | Self::Repo(path) => path.display().to_string(),
        }
    }
}

/// A rule's source code, before it is loaded.
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub origin: Origin,
    pub code: String,
    /// False for a repository rule whose directory is not trusted: it is
    /// listed, never run.
    pub trusted: bool,
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
    pub description: String,
    pub event: Event,
    pub needs: Vec<String>,
    pub state: State,
    table: Option<Table>,
}

/// A tool call, as a rule sees it.
#[derive(Debug, Clone, Default)]
pub struct Call {
    pub event: Option<Event>,
    pub harness: String,
    pub tool: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    /// The command's exit code, for post-tool-use.
    pub exit_code: Option<i32>,
}

impl Call {
    /// The call in a Claude Code or Codex hook payload: `hook_event_name`,
    /// `tool_name`, `tool_input.command` and `cwd`. A `PostToolUseFailure`'s
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
            cwd: text(payload.get("cwd")),
            exit_code,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Test,
    Lint,
    Build,
}

impl Kind {
    fn parse(name: &str) -> Option<Self> {
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

/// A check a post-tool-use rule recorded: a test, lint or build run and how
/// it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    pub rule: String,
    pub kind: Kind,
    pub command: String,
    pub exit_code: i32,
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
    pub time: Duration,
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
            interrupts: 1_000_000,
            memory: 32 * 1024 * 1024,
        }
    }
}

type HasCommand = dyn Fn(&[&str]) -> bool;

pub struct Options {
    /// Whether this jouet has the command at a path such as `["wait", "ci"]`.
    pub has_command: Box<HasCommand>,
    /// Rule names turned off in the settings (`"all"` for every one).
    pub disabled: Vec<String>,
    pub budget: Budget,
    /// Where compiled rules are cached, if anywhere.
    pub cache_dir: Option<PathBuf>,
}

/// What loading a rule needs besides its source.
struct Loader<'a> {
    disabled: &'a [String],
    cache_dir: Option<&'a Path>,
    has_command: &'a HasCommand,
}

/// State the VM's callbacks share with the engine.
struct Shared {
    records: RefCell<Vec<(Kind, String, i32)>>,
    deadline: Cell<Option<Instant>>,
    interrupts: Cell<u32>,
    budget: Budget,
}

pub struct Engine {
    lua: Lua,
    rules: Vec<Rule>,
    shared: Rc<Shared>,
}

/// The built-in rules' sources.
pub fn builtin() -> Vec<Source> {
    BUILTIN
        .iter()
        .map(|(name, code)| Source {
            name: (*name).into(),
            origin: Origin::Builtin,
            code: (*code).into(),
            trusted: true,
        })
        .collect()
}

/// The `*.luau` files in a directory, sorted by name.
pub fn rule_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "luau") && p.is_file())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    files.sort();
    Ok(files)
}

/// Every rule source: built-in, then the user's, then a trusted repository's.
/// A later rule replaces an earlier one with the same name; an untrusted
/// repository rule replaces nothing. Unreadable files are reported, not
/// fatal.
pub fn discover(
    user_dir: Option<&Path>,
    repo_dir: Option<&Path>,
    trust: &TrustStore,
) -> (Vec<Source>, Vec<String>) {
    let mut sources = builtin();
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
                name,
                origin,
                code,
                trusted,
            };
            match sources.iter().position(|s| s.name == source.name) {
                Some(i) if trusted => sources[i] = source,
                _ => sources.push(source),
            }
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
    /// Starts a sandboxed VM and loads every rule. A rule that fails to load
    /// is kept, as [`State::Broken`], so it can be listed.
    pub fn new(sources: Vec<Source>, options: Options) -> mlua::Result<Self> {
        let Options {
            has_command,
            disabled,
            budget,
            cache_dir,
        } = options;
        let lua = Lua::new();
        lua.set_memory_limit(budget.memory)?;
        let shared = Rc::new(Shared {
            records: RefCell::new(Vec::new()),
            deadline: Cell::new(None),
            interrupts: Cell::new(0),
            budget,
        });
        let has_command: Rc<HasCommand> = Rc::from(has_command);
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

        let mut engine = Self {
            lua,
            rules: Vec::new(),
            shared,
        };
        let loader = Loader {
            disabled: &disabled,
            cache_dir: cache_dir.as_deref(),
            has_command: &*has_command,
        };
        for source in sources {
            let rule = engine.load(source, &loader);
            engine.rules.push(rule);
        }
        Ok(engine)
    }

    fn load(&self, source: Source, loader: &Loader<'_>) -> Rule {
        let mut rule = Rule {
            name: source.name,
            origin: source.origin,
            description: String::new(),
            event: Event::PreToolUse,
            needs: Vec::new(),
            state: State::Enabled,
            table: None,
        };
        if !source.trusted {
            rule.state = State::Untrusted;
            return rule;
        }
        let table = match self.load_table(&rule.name, &source.code, loader.cache_dir) {
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
        rule.table = Some(table);
        let needs: Vec<&str> = rule.needs.iter().map(String::as_str).collect();
        if loader
            .disabled
            .iter()
            .any(|d| d == &rule.name || d == "all")
        {
            rule.state = State::Disabled;
        } else if !needs.is_empty() && !(loader.has_command)(&needs) {
            rule.state = State::Needs(format!("jouet {}", needs.join(" ")));
        }
        rule
    }

    /// Compiles (or reads from the cache) and runs a rule file, in an
    /// environment of its own, returning the table it returns.
    fn load_table(
        &self,
        name: &str,
        code: &str,
        cache_dir: Option<&Path>,
    ) -> Result<Table, String> {
        let bytecode = cache::compile(code, cache_dir)?;
        let env = self.lua.create_table().map_err(|e| e.to_string())?;
        let meta = self.lua.create_table().map_err(|e| e.to_string())?;
        meta.set("__index", self.lua.globals())
            .map_err(|e| e.to_string())?;
        env.set_metatable(Some(meta)).map_err(|e| e.to_string())?;
        let value: Value = self.guarded(|| {
            self.lua
                .load(&bytecode)
                .set_name(format!("={name}"))
                .set_mode(mlua::chunk::ChunkMode::Binary)
                .set_environment(env)
                .eval()
        })?;
        match value {
            Value::Table(table) => Ok(table),
            other => Err(format!(
                "the file must return a rule table, not {}",
                other.type_name()
            )),
        }
    }

    /// Runs `f` under the per-rule budget.
    fn guarded<T>(&self, f: impl FnOnce() -> mlua::Result<T>) -> Result<T, String> {
        self.shared.interrupts.set(0);
        self.shared
            .deadline
            .set(Some(Instant::now() + self.shared.budget.time));
        let result = f();
        self.shared.deadline.set(None);
        result.map_err(|e| first_line(&e.to_string()))
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    /// Runs every enabled rule for the call's event, in order, stopping at
    /// the first denial.
    pub fn evaluate(&self, call: &Call) -> Outcome {
        let mut outcome = Outcome::default();
        let event = call.event.unwrap_or(Event::PreToolUse);
        for rule in &self.rules {
            if rule.state != State::Enabled || rule.event != event {
                continue;
            }
            match self.check(rule, call) {
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

    /// Runs one rule's `check`, whatever its state, and returns its decision
    /// and anything it recorded.
    pub fn check(
        &self,
        rule: &Rule,
        call: &Call,
    ) -> Result<(Option<Decision>, Vec<Record>), String> {
        let Some(table) = &rule.table else {
            return Err(match &rule.state {
                State::Broken(e) => e.clone(),
                _ => "the rule is not loaded".into(),
            });
        };
        let check: mlua::Function = table.get("check").map_err(|e| e.to_string())?;
        let call_table = call_table(&self.lua, call).map_err(|e| e.to_string())?;
        self.shared.records.borrow_mut().clear();
        let value: Value = self.guarded(|| check.call(call_table))?;
        let records = self
            .shared
            .records
            .borrow_mut()
            .drain(..)
            .map(|(kind, command, exit_code)| Record {
                rule: rule.name.clone(),
                kind,
                command,
                exit_code,
            })
            .collect();
        let decision = match value {
            Value::Nil => None,
            Value::Table(t) => {
                let action: String = t.get("action").unwrap_or_default();
                let action = match action.as_str() {
                    "deny" => Action::Deny,
                    "warn" => Action::Warn,
                    "allow" => Action::Allow,
                    other => return Err(format!("check returned an unknown action `{other}`")),
                };
                let message: Option<String> = t.get("message").ok().flatten();
                Some(Decision {
                    rule: rule.name.clone(),
                    action,
                    message,
                })
            }
            other => {
                return Err(format!(
                    "check must return jouet.deny(...), jouet.warn(...), jouet.allow() or nil, not {}",
                    other.type_name()
                ));
            }
        };
        Ok((decision, records))
    }

    /// Runs a rule's `examples` and compares each with what it expects.
    pub fn examples(&self, rule: &Rule) -> Result<Vec<Example>, String> {
        let Some(table) = &rule.table else {
            return Err(match &rule.state {
                State::Broken(e) => e.clone(),
                _ => "the rule is not loaded".into(),
            });
        };
        let examples: Option<Table> = table.get("examples").map_err(|e| e.to_string())?;
        let Some(examples) = examples else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for expect in ["deny", "warn", "allow", "record"] {
            let list: Option<Table> = examples.get(expect).map_err(|e| e.to_string())?;
            for case in list.iter().flat_map(|l| l.sequence_values::<Value>()) {
                let case = case.map_err(|e| e.to_string())?;
                let (command, exit_code, kind) = match case {
                    Value::String(s) => (s.to_string_lossy(), 0, None),
                    Value::Table(t) => (
                        t.get::<String>("command").map_err(|e| e.to_string())?,
                        t.get::<Option<i32>>("exit_code")
                            .map_err(|e| e.to_string())?
                            .unwrap_or(0),
                        t.get::<Option<String>>("kind").map_err(|e| e.to_string())?,
                    ),
                    other => {
                        return Err(format!(
                            "examples.{expect} holds a {}, not a command",
                            other.type_name()
                        ));
                    }
                };
                let call = Call {
                    event: Some(rule.event),
                    harness: "test".into(),
                    tool: "Bash".into(),
                    command: Some(command.clone()),
                    cwd: None,
                    exit_code: (rule.event == Event::PostToolUse).then_some(exit_code),
                };
                let expected = match (expect, kind) {
                    ("record", Some(kind)) => format!("record {kind}"),
                    (e, _) => e.to_string(),
                };
                let actual = match self.check(rule, &call) {
                    Ok((decision, records)) => summarize(decision.as_ref(), &records),
                    Err(e) => format!("error: {e}"),
                };
                out.push(Example {
                    ok: actual == expected,
                    command,
                    exit_code: call.exit_code,
                    expected,
                    actual,
                });
            }
        }
        Ok(out)
    }
}

/// One example's result.
#[derive(Debug, Serialize)]
pub struct Example {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub expected: String,
    pub actual: String,
    pub ok: bool,
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

fn read_fields(rule: &mut Rule, table: &Table) -> Result<(), String> {
    let check: Value = table.get("check").map_err(|e| e.to_string())?;
    if !check.is_function() {
        return Err("the rule table needs a `check` function".into());
    }
    rule.description = table
        .get::<Option<String>>("description")
        .map_err(|e| format!("`description`: {e}"))?
        .unwrap_or_default();
    if let Some(on) = table
        .get::<Option<String>>("on")
        .map_err(|e| format!("`on`: {e}"))?
    {
        rule.event = Event::parse(&on).ok_or_else(|| {
            format!("`on` must be \"pre-tool-use\" or \"post-tool-use\", not `{on}`")
        })?;
    }
    rule.needs = table
        .get::<Option<Vec<String>>>("needs")
        .map_err(|e| format!("`needs`: {e}"))?
        .unwrap_or_default();
    Ok(())
}

/// The `jouet` module: the Rust functions, then the prelude's.
fn install_module(
    lua: &Lua,
    shared: &Rc<Shared>,
    has_command: &Rc<HasCommand>,
) -> mlua::Result<()> {
    let jouet = lua.create_table()?;
    jouet.set(
        "parse",
        lua.create_function(|lua, (script, exit_code): (String, Option<i32>)| {
            commands_table(lua, &script, exit_code)
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
        lua.create_function(move |_, (kind, command): (String, Table)| {
            let kind = Kind::parse(&kind).ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "jouet.record: kind must be \"test\", \"lint\" or \"build\", not `{kind}`"
                ))
            })?;
            // A command whose exit status is unknown says nothing.
            if let Some(status) = command.get::<Option<i32>>("status")? {
                let text: String = command.get("text")?;
                records.records.borrow_mut().push((kind, text, status));
            }
            Ok(())
        })?,
    )?;
    lua.load(PRELUDE)
        .set_name("=jouet")
        .call::<()>(jouet.clone())?;
    let globals = lua.globals();
    globals.set("jouet", jouet)?;
    // Rules read the outside world only through `jouet`.
    globals.set("require", Value::Nil)?;
    Ok(())
}

fn call_table(lua: &Lua, call: &Call) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("event", call.event.unwrap_or(Event::PreToolUse).name())?;
    t.set("harness", call.harness.as_str())?;
    t.set("tool", call.tool.as_str())?;
    t.set("command", call.command.as_deref())?;
    t.set("cwd", call.cwd.as_deref())?;
    t.set("exit_code", call.exit_code)?;
    let commands = match &call.command {
        Some(script) => commands_table(lua, script, call.exit_code)?,
        None => lua.create_table()?,
    };
    t.set("commands", commands)?;
    Ok(t)
}

/// Every command a script runs, as tables: the top-level ones with their
/// exit status when it is known, then those inside `$(...)`, then those in
/// scripts passed to `sh -c` and the like.
fn commands_table(lua: &Lua, script: &str, exit_code: Option<i32>) -> mlua::Result<Table> {
    let parsed = shell::parse(script);
    let statuses = match exit_code {
        Some(code) => shell::statuses(&parsed.commands, code),
        None => vec![None; parsed.commands.len()],
    };
    let list = lua.create_table()?;
    for (command, status) in parsed.commands.iter().zip(statuses) {
        list.push(command_table(lua, command, false, status)?)?;
    }
    for command in &parsed.nested {
        list.push(command_table(lua, command, true, None)?)?;
    }
    let mut queue = parsed.inner_scripts();
    let mut seen = 0;
    while let Some(inner) = queue.pop() {
        seen += 1;
        if seen > 16 {
            break;
        }
        for command in inner.all() {
            list.push(command_table(lua, command, true, None)?)?;
        }
        queue.extend(inner.inner_scripts());
    }
    Ok(list)
}

fn command_table(
    lua: &Lua,
    command: &shell::Command,
    nested: bool,
    status: Option<i32>,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let argv = command.argv();
    t.set("words", command.words.clone())?;
    t.set("argv", argv.to_vec())?;
    t.set("program", command.program())?;
    t.set("heredocs", command.heredocs.clone())?;
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
    t.set("nested", nested)?;
    t.set("status", status)?;
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
