//! Recipes: Luau scripts that sequence jouet's own commands, run by
//! `jouet run`.
//!
//! A recipe is a `.luau` file with a `--- why:` header that returns a table
//! with a `run(args)` function. It runs in the same sandbox as a rule, with
//! the same `jouet` module and probes, plus `jouet.cmd`: one function per
//! command a recipe may run, such as `jouet.cmd.wait_ci{ pr = "123" }`. Each
//! returns the command's JSON document and exit code. There is no other way
//! out: no shell, no files.
//!
//! The guardrails:
//! - Only the commands in [`COMMANDS`] are there, so a recipe can do nothing
//!   its commands could not. `wait cmd` and `edit --check`, which run shell
//!   commands, and `rule trust`, which is for a person, are not among them.
//!   `test` runs the repository's own test task, which jouet detects.
//! - A command that posts or merges runs only with `yes = true` (or with
//!   `dry_run = true`, which it honours by changing nothing), and still
//!   refuses whatever it refuses when run by hand.
//! - The budget applies per step: the Luau code and probes between two
//!   commands get [`Budget::recipe`], and a command's own running time is
//!   not counted. A run starts at most [`MAX_STEPS`] commands.
//! - A repository's recipes run only once it is trusted, like its rules.

use crate::json::{from_lua, to_lua};
use crate::{Budget, HasCommand, Origin, Probes, Scope, Source, TrustStore, Vm, header};
use mlua::{Function, Lua, Table, Value};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// The built-in recipes, embedded in the binary.
const BUILTIN: &[(&str, &str)] = &[
    ("land", include_str!("../recipes/land.luau")),
    ("ship-check", include_str!("../recipes/ship-check.luau")),
    ("sweep", include_str!("../recipes/sweep.luau")),
];

/// The most commands one run may start.
pub const MAX_STEPS: usize = 64;

impl Budget {
    /// A recipe's budget for each step: the code and probes between two
    /// commands. Larger than a rule's, since a recipe's probes may go to
    /// GitHub.
    pub fn recipe() -> Self {
        Self {
            time: Duration::from_secs(30),
            interrupts: 10_000_000,
            memory: 64 * 1024 * 1024,
        }
    }
}

/// What running a command does to the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Only reads, or waits.
    Read,
    /// Runs the repository's test task.
    Test,
    /// Posts to GitHub or merges: needs `yes = true`.
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    /// `true` passes `--name`.
    Switch,
    /// `--name=value`.
    Value,
    /// `--name=value` for each value of a list (or for one value).
    Repeat,
}

use Flag::{Repeat, Switch, Value as Val};

/// A command a recipe may run, and the arguments it takes. Names are the
/// command's own, with `_` for `-`: `log_lines` is `--log-lines`.
#[derive(Debug)]
pub struct Command {
    pub name: &'static str,
    /// The jouet command, such as `["pr", "land"]`; empty for `test`.
    pub path: &'static [&'static str],
    pub effect: Effect,
    /// Positional arguments in order, and whether each takes many values.
    positionals: &'static [(&'static str, bool)],
    flags: &'static [&'static [(&'static str, Flag)]],
    /// Words a positional argument may not be, because the command would
    /// read them as a subcommand.
    subcommands: &'static [&'static str],
    /// `yes = true` is passed on as `--yes`.
    yes_flag: bool,
}

const WAIT: &[(&str, Flag)] = &[("timeout", Val), ("interval", Val), ("quiet", Switch)];
const PR_LIST: &[(&str, Flag)] = &[
    ("mine", Switch),
    ("search", Val),
    ("repo", Repeat),
    ("settle", Val),
];

const fn read(
    name: &'static str,
    path: &'static [&'static str],
    positionals: &'static [(&'static str, bool)],
    flags: &'static [&'static [(&'static str, Flag)]],
) -> Command {
    Command {
        name,
        path,
        effect: Effect::Read,
        positionals,
        flags,
        subcommands: &[],
        yes_flag: false,
    }
}

/// Every command `jouet.cmd` has.
pub const COMMANDS: &[Command] = &[
    read(
        "pr_status",
        &["pr", "status"],
        &[("prs", true)],
        &[PR_LIST, &[("fail_on_attention", Switch)]],
    ),
    read("pr_next", &["pr", "next"], &[("prs", true)], &[PR_LIST]),
    Command {
        subcommands: &["reply", "resolve", "unresolve", "apply"],
        ..read(
            "pr_threads",
            &["pr", "threads"],
            &[("pr", false)],
            &[&[
                ("repo", Val),
                ("all", Switch),
                ("resolved", Switch),
                ("outdated", Switch),
                ("no_outdated", Switch),
                ("author", Repeat),
                ("bots", Switch),
                ("no_bots", Switch),
                ("since", Val),
                ("path", Repeat),
                ("include_reviews", Switch),
                ("context", Val),
                ("full", Switch),
                ("fail_if_unresolved", Switch),
            ]],
        )
    },
    Command {
        effect: Effect::Write,
        ..read(
            "pr_reply",
            &["pr", "threads", "reply"],
            &[("id", false), ("pr", false)],
            &[&[
                ("repo", Val),
                ("body", Val),
                ("resolve", Switch),
                ("ignore_resolved", Switch),
                ("dry_run", Switch),
            ]],
        )
    },
    Command {
        effect: Effect::Write,
        ..read(
            "pr_resolve",
            &["pr", "threads", "resolve"],
            &[("ids", true)],
            &[&[
                ("repo", Val),
                ("reply", Val),
                ("ignore_resolved", Switch),
                ("dry_run", Switch),
            ]],
        )
    },
    Command {
        effect: Effect::Write,
        yes_flag: true,
        ..read(
            "pr_land",
            &["pr", "land"],
            &[("pr", false)],
            &[&[
                ("repo", Val),
                ("method", Val),
                ("update", Switch),
                ("dry_run", Switch),
                ("timeout", Val),
                ("interval", Val),
                ("settle", Val),
            ]],
        )
    },
    read(
        "wait_ci",
        &["wait", "ci"],
        &[("pr", false)],
        &[
            WAIT,
            &[
                ("repo", Val),
                ("sha", Val),
                ("required", Switch),
                ("check", Repeat),
                ("ignore", Repeat),
                ("all", Switch),
                ("log_lines", Val),
                ("no_logs", Switch),
                ("appear_timeout", Val),
            ],
        ],
    ),
    read(
        "wait_run",
        &["wait", "run"],
        &[("run", false)],
        &[
            WAIT,
            &[("repo", Val), ("log_lines", Val), ("no_logs", Switch)],
        ],
    ),
    read(
        "wait_pr",
        &["wait", "pr"],
        &[("pr", false)],
        &[WAIT, &[("repo", Val), ("until", Val)]],
    ),
    read(
        "wait_file",
        &["wait", "file"],
        &[("path", false)],
        &[WAIT, &[("contains", Val), ("gone", Switch)]],
    ),
    read(
        "wait_log",
        &["wait", "log"],
        &[("path", false)],
        &[WAIT, &[("match", Val), ("from_start", Switch)]],
    ),
    read(
        "wait_port",
        &["wait", "port"],
        &[("addr", false)],
        &[WAIT, &[("closed", Switch)]],
    ),
    read(
        "wait_http",
        &["wait", "http"],
        &[("url", false)],
        &[WAIT, &[("status", Val), ("contains", Val)]],
    ),
    read("wait_pid", &["wait", "pid"], &[("pid", false)], &[WAIT]),
    Command {
        effect: Effect::Test,
        ..read("test", &[], &[], &[])
    },
];

/// One command a recipe asked for.
#[derive(Debug, Clone)]
pub struct Step {
    pub command: &'static Command,
    /// The arguments after the command's path.
    pub args: Vec<String>,
}

impl Step {
    /// `jouet --json PATH... ARGS...` without the `jouet`, for a jouet
    /// command; None for the test task.
    pub fn argv(&self) -> Option<Vec<String>> {
        if self.command.path.is_empty() {
            return None;
        }
        let mut argv = vec!["--json".to_string()];
        argv.extend(self.command.path.iter().map(|w| (*w).to_string()));
        argv.extend(self.args.iter().cloned());
        Some(argv)
    }
}

/// What a command printed and how it exited.
#[derive(Debug, Clone)]
pub struct StepOutput {
    pub exit_code: i32,
    /// Its JSON document, when it printed one.
    pub json: Option<serde_json::Value>,
}

/// What runs a recipe's steps: the binary, or a test's stub.
pub trait Host {
    /// The command line a step runs, for progress and for dry runs.
    fn describe(&self, step: &Step) -> String;
    /// Runs the step.
    fn run(&mut self, step: &Step) -> Result<StepOutput, String>;
}

#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub cmd: &'static str,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The state of a run, which `jouet.cmd` reads.
pub(crate) struct RunState {
    host: Box<dyn Host>,
    dry_run: bool,
    steps: Vec<StepReport>,
    /// A guardrail stopped the run.
    refused: bool,
}

/// Whether a recipe can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    /// A repository recipe in a directory that is not trusted.
    Untrusted,
    Broken(String),
}

pub struct Recipe {
    pub name: String,
    pub origin: Origin,
    pub why: String,
    /// The table's `usage`: the arguments it takes, in a line.
    pub usage: Option<String>,
    pub state: State,
    table: Option<Table>,
}

/// What a run did.
#[derive(Debug, Serialize)]
pub struct Report {
    pub recipe: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub steps: Vec<StepReport>,
    /// The table the recipe returned, as JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct Options {
    /// Whether this jouet has the command at a path such as `["wait", "ci"]`.
    pub has_command: Box<HasCommand>,
    /// Where compiled recipes are cached, if anywhere.
    pub cache_dir: Option<PathBuf>,
}

/// The built-in recipes' sources.
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

/// Every recipe source: built-in, then the user's, then a repository's
/// (untrusted unless its directory is trusted).
pub fn discover(
    user_dir: Option<&Path>,
    repo_dir: Option<&Path>,
    trust: &TrustStore,
) -> (Vec<Source>, Vec<String>) {
    crate::discover_after(builtin(), user_dir, repo_dir, trust)
}

pub struct Recipes {
    vm: Vm,
    recipes: Vec<Recipe>,
}

impl Recipes {
    /// Starts a VM and loads every recipe. One that fails to load is kept,
    /// as [`State::Broken`], so it can be listed. As with rules, a recipe
    /// of yours or a repository's never takes a built-in one's name.
    pub fn new(sources: Vec<Source>, options: Options) -> mlua::Result<Self> {
        let vm = Vm::new(Budget::recipe(), Rc::from(options.has_command))?;
        let mut recipes: Vec<Recipe> = Vec::new();
        for source in sources {
            let mut recipe = load(&vm, source, options.cache_dir.as_deref());
            if recipe.state != State::Untrusted
                && let Some(other) = recipes
                    .iter()
                    .find(|r| r.name == recipe.name && r.state != State::Untrusted)
            {
                recipe.state = State::Broken(match other.origin.scope() {
                    Scope::Builtin => format!(
                        "`{}` is a built-in recipe, which cannot be replaced; rename this file",
                        recipe.name
                    ),
                    _ => format!(
                        "{} already defines `{}`",
                        other.origin.describe(),
                        recipe.name
                    ),
                });
                recipe.table = None;
            }
            recipes.push(recipe);
        }
        Ok(Self { vm, recipes })
    }

    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }

    /// The recipe that runs under `name`: a ready one first.
    pub fn get(&self, name: &str) -> Option<&Recipe> {
        let named = || self.recipes.iter().filter(move |r| r.name == name);
        named()
            .find(|r| r.state == State::Ready)
            .or_else(|| named().next())
    }

    /// Runs a recipe with the words after its name: `key=value` sets
    /// `args.key` (`true` and `false` are booleans), anything else is the
    /// next of `args[1]`, `args[2]`, .... With `dry_run`, no command runs:
    /// each returns `{ ok = true, exit_code = 0, dry_run = true, json = {} }`
    /// and is listed.
    pub fn run(
        &self,
        recipe: &Recipe,
        words: &[String],
        probes: Probes,
        host: Box<dyn Host>,
        dry_run: bool,
    ) -> Report {
        let mut report = Report {
            recipe: recipe.name.clone(),
            ok: false,
            dry_run,
            exit_code: 1,
            message: None,
            steps: Vec::new(),
            result: None,
            error: None,
        };
        let Some(table) = &recipe.table else {
            report.error = Some(match &recipe.state {
                State::Broken(e) => e.clone(),
                _ => {
                    "the recipe's directory is not trusted; review it, then run `jouet run --trust`"
                        .into()
                }
            });
            report.exit_code = 4;
            return report;
        };
        let shared = &self.vm.shared;
        *shared.probes.borrow_mut() = Some(probes);
        *shared.recipe.borrow_mut() = Some(RunState {
            host,
            dry_run,
            steps: Vec::new(),
            refused: false,
        });
        self.vm.new_call();
        let returned = self.vm.guarded(|| {
            let run: Function = table.raw_get("run")?;
            let args = args_table(&self.vm.lua, words)?;
            let value = run.call::<Value>(args)?;
            // Read inside the budget: the table is the recipe's.
            Ok(match &value {
                Value::Nil => Ok((true, None, None, None)),
                Value::String(s) => Ok((true, Some(s.to_string_lossy()), None, None)),
                Value::Table(t) => Ok((
                    t.raw_get::<Option<bool>>("ok")?.unwrap_or(true),
                    t.raw_get::<Option<String>>("message")?,
                    t.raw_get::<Option<i64>>("exit_code")?,
                    Some(from_lua(&value).map_err(mlua::Error::runtime)?),
                )),
                other => Err(other.type_name()),
            })
        });
        let state = shared.recipe.borrow_mut().take();
        let refused = state.as_ref().is_some_and(|s| s.refused);
        report.steps = state.map(|s| s.steps).unwrap_or_default();
        match returned {
            Ok(Ok((ok, message, exit_code, result))) => {
                report.ok = ok;
                report.message = message;
                report.result = result;
                report.exit_code = exit_code
                    .and_then(|c| i32::try_from(c).ok())
                    .filter(|c| (0..=255).contains(c))
                    .unwrap_or(if ok { 0 } else { 1 });
            }
            Ok(Err(type_name)) => {
                report.error = Some(format!(
                    "run must return nil, a string or a table, not {type_name}"
                ));
            }
            Err(e) => {
                report.error = Some(e);
                if refused {
                    report.exit_code = 4;
                }
            }
        }
        report
    }
}

/// Compiles a recipe and reads its table.
fn load(vm: &Vm, source: Source, cache_dir: Option<&Path>) -> Recipe {
    let header = header::parse(&source.code);
    let mut recipe = Recipe {
        name: source.name,
        origin: source.origin,
        why: header.why.clone().unwrap_or_default(),
        usage: None,
        state: State::Ready,
        table: None,
    };
    if header.why.is_none() {
        recipe.state = State::Broken("start the file with a `--- why: ...` header line".into());
        return recipe;
    }
    if !source.trusted {
        recipe.state = State::Untrusted;
        return recipe;
    }
    let read = vm
        .load_table(&recipe.name, &source.code, cache_dir)
        .and_then(|table| {
            let run: Value = table.raw_get("run").map_err(|e| e.to_string())?;
            if !run.is_function() {
                return Err("the recipe table needs a `run` function".into());
            }
            let usage = table
                .raw_get::<Option<String>>("usage")
                .map_err(|e| format!("`usage`: {e}"))?;
            Ok((table, usage))
        });
    match read {
        Ok((table, usage)) => {
            recipe.usage = usage;
            recipe.table = Some(table);
        }
        Err(e) => recipe.state = State::Broken(e),
    }
    recipe
}

/// `args` for `run`: `key=value` words as fields, the rest in order.
fn args_table(lua: &Lua, words: &[String]) -> mlua::Result<Table> {
    let args = lua.create_table()?;
    for word in words {
        let field = word.split_once('=').filter(|(key, _)| {
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        match field {
            Some((key, "true")) => args.raw_set(key, true)?,
            Some((key, "false")) => args.raw_set(key, false)?,
            Some((key, value)) => args.raw_set(key, value)?,
            None => args.raw_push(word.as_str())?,
        }
    }
    Ok(args)
}

/// The arguments a `jouet.cmd` call was given, checked against the
/// command.
struct Built {
    args: Vec<String>,
    yes: bool,
    dry_run: bool,
}

/// A value as one command-line word.
fn word(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.to_string_lossy()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => Some(format!("{n:.0}")),
        _ => None,
    }
}

/// A value as a list of words: one word, or a sequence of them.
fn words(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Table(t) => t
            .sequence_values::<Value>()
            .map(|v| v.ok().as_ref().and_then(word))
            .collect(),
        other => word(other).map(|w| vec![w]),
    }
}

fn build(command: &Command, args: Option<&Table>) -> Result<Built, String> {
    let mut indexed: BTreeMap<i64, Value> = BTreeMap::new();
    let mut named: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(args) = args {
        for pair in args.pairs::<Value, Value>() {
            let (key, value) = pair.map_err(|e| e.to_string())?;
            match key {
                Value::Integer(i) => {
                    indexed.insert(i, value);
                }
                Value::String(s) => {
                    named.insert(s.to_string_lossy(), value);
                }
                other => {
                    return Err(format!(
                        "an argument's key cannot be a {}",
                        other.type_name()
                    ));
                }
            }
        }
    }
    let expected = || {
        let mut names: Vec<&str> = command.positionals.iter().map(|(n, _)| *n).collect();
        names.extend(command.flags.iter().flat_map(|f| f.iter().map(|(n, _)| *n)));
        if command.effect == Effect::Write {
            names.push("yes");
        }
        if names.is_empty() {
            "it takes no arguments".to_string()
        } else {
            format!("it takes {}", names.join(", "))
        }
    };
    let bool_arg = |named: &mut BTreeMap<String, Value>, key: &str| match named.remove(key) {
        None | Some(Value::Nil) => Ok(false),
        Some(Value::Boolean(b)) => Ok(b),
        Some(other) => Err(format!(
            "`{key}` must be true or false, not a {}",
            other.type_name()
        )),
    };
    let yes = if command.effect == Effect::Write {
        bool_arg(&mut named, "yes")?
    } else {
        false
    };
    let mut positional_values: Vec<Value> = indexed.into_values().collect();
    positional_values.reverse();
    let mut out = Vec::new();
    for (name, many) in command.positionals {
        let mut values = Vec::new();
        if let Some(value) = named.remove(*name) {
            values = words(&value).ok_or_else(|| {
                format!(
                    "`{name}` must be a string or number{}",
                    if *many { ", or a list of them" } else { "" }
                )
            })?;
        } else if *many {
            while let Some(value) = positional_values.pop() {
                values.push(word(&value).ok_or("positional arguments must be strings or numbers")?);
            }
        } else if let Some(value) = positional_values.pop() {
            values.push(word(&value).ok_or("positional arguments must be strings or numbers")?);
        }
        if !*many && values.len() > 1 {
            return Err(format!("`{name}` takes one value"));
        }
        for value in values {
            if value.starts_with('-') {
                return Err(format!("`{name}` cannot start with `-`: `{value}`"));
            }
            if command.subcommands.contains(&value.as_str()) {
                return Err(format!(
                    "`{name}` cannot be `{value}`, which this command reads as a subcommand"
                ));
            }
            out.push(value);
        }
    }
    if !positional_values.is_empty() {
        return Err(format!("too many positional arguments; {}", expected()));
    }
    let mut dry_run = false;
    for (name, flag) in command.flags.iter().flat_map(|f| f.iter()) {
        let long = format!("--{}", name.replace('_', "-"));
        if *flag == Switch {
            let on = bool_arg(&mut named, name)?;
            if on {
                out.push(long);
            }
            if *name == "dry_run" {
                dry_run = on;
            }
            continue;
        }
        let Some(value) = named.remove(*name) else {
            continue;
        };
        let values = match flag {
            Repeat => words(&value),
            _ => word(&value).map(|w| vec![w]),
        }
        .ok_or_else(|| format!("`{name}` must be a string or number"))?;
        for value in values {
            out.push(format!("{long}={value}"));
        }
    }
    if let Some(name) = named.keys().next() {
        return Err(format!("no argument `{name}`; {}", expected()));
    }
    if yes && command.yes_flag {
        out.push("--yes".into());
    }
    Ok(Built {
        args: out,
        yes,
        dry_run,
    })
}

/// A step's result as Luau sees it.
fn result_table(
    lua: &Lua,
    command: &str,
    output: &StepOutput,
    dry_run: bool,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.raw_set("command", command)?;
    t.raw_set("exit_code", output.exit_code)?;
    t.raw_set("ok", output.exit_code == 0)?;
    if dry_run {
        t.raw_set("dry_run", true)?;
    }
    match &output.json {
        Some(json) => {
            if let Some(message) = json["error"]["message"].as_str() {
                t.raw_set("error", message)?;
            }
            t.raw_set("json", to_lua(lua, json)?)?;
        }
        None => t.raw_set("json", lua.create_table()?)?,
    }
    Ok(t)
}

/// `jouet.cmd`: one function per command in [`COMMANDS`]. Outside a recipe
/// each one is an error.
pub(crate) fn install(
    lua: &Lua,
    jouet: &Table,
    shared: &Rc<crate::Shared>,
    has_command: &Rc<HasCommand>,
) -> mlua::Result<()> {
    let cmd = lua.create_table()?;
    for command in COMMANDS {
        let shared = shared.clone();
        let has_command = has_command.clone();
        let function = lua.create_function(move |lua, args: Option<Table>| {
            let fail = |message: String| mlua::Error::runtime(format!("jouet.cmd.{}: {message}", command.name));
            let mut guard = shared.recipe.borrow_mut();
            let Some(state) = guard.as_mut() else {
                return Err(fail("commands run from recipes (`jouet run`), not from rules".into()));
            };
            if !command.path.is_empty() && !has_command(command.path) {
                return Err(fail(format!("this jouet has no `jouet {}`", command.path.join(" "))));
            }
            let built = build(command, args.as_ref()).map_err(fail)?;
            if command.effect == Effect::Write && !built.yes && !built.dry_run {
                state.refused = true;
                return Err(fail(
                    "this command posts or merges; pass `yes = true` to let it, or `dry_run = true` to see what it would do"
                        .into(),
                ));
            }
            if state.steps.len() >= MAX_STEPS {
                return Err(fail(format!("a recipe runs at most {MAX_STEPS} commands")));
            }
            let step = Step {
                command,
                args: built.args,
            };
            let described = state.host.describe(&step);
            if state.dry_run {
                state.steps.push(StepReport {
                    cmd: command.name,
                    command: described.clone(),
                    exit_code: None,
                    error: None,
                });
                let output = StepOutput {
                    exit_code: 0,
                    json: None,
                };
                return result_table(lua, &described, &output, true);
            }
            // The command's own running time is not the recipe's.
            shared.deadline.set(None);
            let ran = state.host.run(&step);
            shared.cache.borrow_mut().clear();
            shared.restart_budget();
            match ran {
                Ok(output) => {
                    state.steps.push(StepReport {
                        cmd: command.name,
                        command: described.clone(),
                        exit_code: Some(output.exit_code),
                        error: output.json.as_ref().and_then(|j| j["error"]["message"].as_str()).map(str::to_string),
                    });
                    result_table(lua, &described, &output, false)
                }
                Err(e) => {
                    state.steps.push(StepReport {
                        cmd: command.name,
                        command: described,
                        exit_code: None,
                        error: Some(e.clone()),
                    });
                    Err(fail(e))
                }
            }
        })?;
        cmd.raw_set(command.name, function)?;
    }
    jouet.set("cmd", cmd)?;
    Ok(())
}

#[cfg(test)]
mod tests;
