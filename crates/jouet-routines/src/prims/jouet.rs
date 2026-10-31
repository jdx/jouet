//! `wait` and `jouet`: jouet's own commands, run as `jouet --json ...` by
//! the host, so a routine gets exactly what the command does by hand, its
//! progress on stderr and its JSON document back.
//!
//! - `wait.ci{pr}`, `wait.pr{pr, until}`, `wait.run{run}`, `wait.file{path}`,
//!   `wait.log{path, match}`, `wait.port{addr}`, `wait.http{url}` and
//!   `wait.pid{pid}`: `jouet wait`'s engine.
//! - `jouet.pr_status`, `jouet.pr_next`, `jouet.pr_threads`, and the writes
//!   `jouet.pr_reply`, `jouet.pr_resolve` and `jouet.pr_land`.
//! - `jouet.code.symbols{query}`, `jouet.code.callers{symbol}`,
//!   `jouet.code.callees{symbol}`, `jouet.code.impact{symbol}`,
//!   `jouet.code.explore{query}` and `jouet.code.affected{files}`: the code
//!   graph's queries (`jouet graph`), with `budget` and `page` to page a long
//!   answer.
//!
//! Each takes a table: positional arguments in order (or by name), flags by
//! name with `_` for `-`. Each returns `{ ok, exit_code, json, error,
//! command }` rather than raising, since a wait that times out or a land
//! that is refused is an answer. A write runs with `--yes` when the routine
//! was given `--yes`, and with `--dry-run`, which previews it, otherwise.
//! `wait cmd` and `edit --check`, which run shell commands, are not here.

use super::primitive;
use crate::Fail;
use crate::capability::Capability::{self, FsRead, GithubRead, GithubWrite, Net};
use crate::runtime::{Ctx, to_lua};
use crate::world::StepOutput;
use jouet_output::exit::Exit;
use mlua::{Lua, Table, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

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

/// A command a routine may run, and the arguments it takes.
#[derive(Debug)]
pub struct Command {
    /// The table and key: `("wait", "ci")` is `wait.ci`.
    pub table: &'static str,
    pub key: &'static str,
    /// The jouet command, such as `["pr", "land"]`.
    pub path: &'static [&'static str],
    pub capability: Option<Capability>,
    /// Posts or merges: `--yes` with the routine's `--yes`, else
    /// `--dry-run`.
    pub writes: bool,
    /// Positional arguments in order, and whether each takes many values.
    positionals: &'static [(&'static str, bool)],
    flags: &'static [&'static [(&'static str, Flag)]],
    /// Words a positional argument may not be, because the command would
    /// read them as a subcommand.
    subcommands: &'static [&'static str],
    /// Takes `--yes` (as `pr land` does) rather than acting by default.
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
    table: &'static str,
    key: &'static str,
    path: &'static [&'static str],
    capability: Capability,
    positionals: &'static [(&'static str, bool)],
    flags: &'static [&'static [(&'static str, Flag)]],
) -> Command {
    Command {
        table,
        key,
        path,
        capability: Some(capability),
        writes: false,
        positionals,
        flags,
        subcommands: &[],
        yes_flag: false,
    }
}

/// Every command `wait` and `jouet` have.
/// The flags `jouet graph callers`, `callees` and `impact` share.
const CODE_TRAVERSE: &[(&str, Flag)] = &[("depth", Val), ("budget", Val), ("page", Val)];

pub const COMMANDS: &[Command] = &[
    read(
        "jouet",
        "pr_status",
        &["pr", "status"],
        GithubRead,
        &[("prs", true)],
        &[PR_LIST, &[("fail_on_attention", Switch)]],
    ),
    read(
        "jouet",
        "pr_next",
        &["pr", "next"],
        GithubRead,
        &[("prs", true)],
        &[PR_LIST],
    ),
    Command {
        subcommands: &["reply", "resolve", "unresolve", "apply"],
        ..read(
            "jouet",
            "pr_threads",
            &["pr", "threads"],
            GithubRead,
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
        writes: true,
        ..read(
            "jouet",
            "pr_reply",
            &["pr", "threads", "reply"],
            GithubWrite,
            &[("id", false), ("pr", false)],
            &[&[
                ("repo", Val),
                ("body", Val),
                ("resolve", Switch),
                ("ignore_resolved", Switch),
            ]],
        )
    },
    Command {
        writes: true,
        ..read(
            "jouet",
            "pr_resolve",
            &["pr", "threads", "resolve"],
            GithubWrite,
            &[("ids", true)],
            &[&[("repo", Val), ("reply", Val), ("ignore_resolved", Switch)]],
        )
    },
    Command {
        writes: true,
        yes_flag: true,
        ..read(
            "jouet",
            "pr_land",
            &["pr", "land"],
            GithubWrite,
            &[("pr", false)],
            &[&[
                ("repo", Val),
                ("method", Val),
                ("update", Switch),
                ("timeout", Val),
                ("interval", Val),
                ("settle", Val),
            ]],
        )
    },
    // `jouet.code`: the code graph. Each query refreshes it first.
    read(
        "jouet.code",
        "symbols",
        &["graph", "symbols"],
        FsRead,
        &[("query", false)],
        &[&[("kind", Val), ("path", Val), ("budget", Val), ("page", Val)]],
    ),
    read(
        "jouet.code",
        "callers",
        &["graph", "callers"],
        FsRead,
        &[("symbol", false)],
        &[CODE_TRAVERSE],
    ),
    read(
        "jouet.code",
        "callees",
        &["graph", "callees"],
        FsRead,
        &[("symbol", false)],
        &[CODE_TRAVERSE],
    ),
    read(
        "jouet.code",
        "impact",
        &["graph", "impact"],
        FsRead,
        &[("symbol", false)],
        &[CODE_TRAVERSE],
    ),
    read(
        "jouet.code",
        "explore",
        &["graph", "explore"],
        FsRead,
        &[("query", true)],
        &[&[("budget", Val), ("source_budget", Val), ("page", Val)]],
    ),
    read(
        "jouet.code",
        "affected",
        &["graph", "affected"],
        FsRead,
        &[("files", true)],
        &[],
    ),
    read(
        "wait",
        "ci",
        &["wait", "ci"],
        GithubRead,
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
        "wait",
        "run",
        &["wait", "run"],
        GithubRead,
        &[("run", false)],
        &[
            WAIT,
            &[("repo", Val), ("log_lines", Val), ("no_logs", Switch)],
        ],
    ),
    read(
        "wait",
        "pr",
        &["wait", "pr"],
        GithubRead,
        &[("pr", false)],
        &[WAIT, &[("repo", Val), ("until", Val)]],
    ),
    read(
        "wait",
        "file",
        &["wait", "file"],
        FsRead,
        &[("path", false)],
        &[WAIT, &[("contains", Val), ("gone", Switch)]],
    ),
    read(
        "wait",
        "log",
        &["wait", "log"],
        FsRead,
        &[("path", false)],
        &[WAIT, &[("match", Val), ("from_start", Switch)]],
    ),
    read(
        "wait",
        "port",
        &["wait", "port"],
        Net,
        &[("addr", false)],
        &[WAIT, &[("closed", Switch)]],
    ),
    read(
        "wait",
        "http",
        &["wait", "http"],
        Net,
        &[("url", false)],
        &[WAIT, &[("status", Val), ("contains", Val)]],
    ),
    Command {
        capability: None,
        ..read(
            "wait",
            "pid",
            &["wait", "pid"],
            Net,
            &[("pid", false)],
            &[WAIT],
        )
    },
];

impl Command {
    /// `wait.ci` or `jouet.pr_land`.
    pub fn name(&self) -> String {
        format!("{}.{}", self.table, self.key)
    }
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

/// The command's arguments after its path, from the table a routine gave.
pub(crate) fn build(command: &Command, args: Option<&Table>) -> Result<Vec<String>, String> {
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
        if names.is_empty() {
            "it takes no arguments".to_string()
        } else {
            format!("it takes {}", names.join(", "))
        }
    };
    let mut positional_values: Vec<Value> = indexed.into_values().collect();
    positional_values.reverse();
    let mut out = Vec::new();
    for (name, many) in command.positionals {
        let mut values = Vec::new();
        if let Some(value) = named.remove(*name) {
            if value != Value::Nil {
                values = words(&value).ok_or_else(|| {
                    format!(
                        "`{name}` must be a string or number{}",
                        if *many { ", or a list of them" } else { "" }
                    )
                })?;
            }
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
    for (name, flag) in command.flags.iter().flat_map(|f| f.iter()) {
        let long = format!("--{}", name.replace('_', "-"));
        let Some(value) = named.remove(*name) else {
            continue;
        };
        if *flag == Switch {
            match value {
                Value::Nil | Value::Boolean(false) => {}
                Value::Boolean(true) => out.push(long),
                other => {
                    return Err(format!(
                        "`{name}` must be true or false, not a {}",
                        other.type_name()
                    ));
                }
            }
            continue;
        }
        if value == Value::Nil {
            continue;
        }
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
        let hint = if ["yes", "dry_run"].contains(&name.as_str()) {
            "; a write follows the routine's own --yes and --dry-run".to_string()
        } else {
            String::new()
        };
        return Err(format!("no argument `{name}`; {}{hint}", expected()));
    }
    Ok(out)
}

/// A word as a shell would need it, for showing a command line.
pub(crate) fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=./:@#,+%".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// A command's result as Luau sees it.
fn result_table(lua: &Lua, command: &str, output: &StepOutput) -> mlua::Result<Value> {
    let mut value = serde_json::json!({
        "command": command,
        "exit_code": output.exit_code,
        "ok": output.exit_code == 0,
        "json": output.json.clone().unwrap_or_else(|| serde_json::json!({})),
    });
    if let Some(message) = output
        .json
        .as_ref()
        .and_then(|j| j["error"]["message"].as_str())
    {
        value["error"] = message.into();
    }
    to_lua(lua, &value)
}

pub(super) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let wait = lua.create_table()?;
    let jouet = lua.create_table()?;
    let code = lua.create_table()?;
    for command in COMMANDS {
        let name = command.name();
        let f = primitive(
            lua,
            ctx,
            command.capability,
            name.clone(),
            move |lua, ctx, args: Option<Table>| {
                let mut argv: Vec<String> = command.path.iter().map(|w| (*w).to_string()).collect();
                argv.extend(
                    build(command, args.as_ref())
                        .map_err(|e| mlua::Error::runtime(format!("{name}: {e}")))?,
                );
                let shown = format!(
                    "jouet {}",
                    argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
                );
                if command.writes {
                    if ctx.write(shown.clone()) {
                        if command.yes_flag {
                            argv.push("--yes".into());
                        }
                    } else {
                        argv.push("--dry-run".into());
                    }
                }
                let ran = ctx.world.borrow_mut().host.jouet(&argv);
                match ran {
                    Ok(output) => result_table(lua, &shown, &output),
                    Err(e) => Err(Fail::new(Exit::Failed, format!("{name}: {e}")).raise()),
                }
            },
        )?;
        match command.table {
            "wait" => wait.set(command.key, f)?,
            "jouet.code" => code.set(command.key, f)?,
            _ => jouet.set(command.key, f)?,
        }
    }
    jouet.set("code", code)?;
    globals.set("wait", wait)?;
    globals.set("jouet", jouet)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(key: &str, code: &str) -> Result<Vec<String>, String> {
        let lua = Lua::new();
        let command = COMMANDS.iter().find(|c| c.key == key).unwrap();
        let args: Option<Table> = lua.load(code).eval().unwrap();
        build(command, args.as_ref())
    }

    #[test]
    fn arguments_become_the_commands_words() {
        assert_eq!(
            built(
                "ci",
                r#"return { "123", repo = "jdx/demo", check = { "lint", "test" }, all = true }"#
            )
            .unwrap(),
            [
                "123",
                "--repo=jdx/demo",
                "--check=lint",
                "--check=test",
                "--all"
            ]
        );
        assert_eq!(
            built("pr_next", r#"return { prs = { 1, 2 }, mine = true }"#).unwrap(),
            ["1", "2", "--mine"]
        );
        assert_eq!(
            built("pr_next", "return nil").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            built("callers", r#"return { "src/a.rs#run", depth = 2 }"#).unwrap(),
            ["src/a.rs#run", "--depth=2"]
        );
        assert_eq!(
            built(
                "explore",
                r#"return { query = { "retry", "backoff" }, budget = 2000 }"#
            )
            .unwrap(),
            ["retry", "backoff", "--budget=2000"]
        );
    }

    #[test]
    fn bad_arguments_are_refused_with_what_it_takes() {
        assert!(
            built("ci", r#"return { nope = 1 }"#)
                .unwrap_err()
                .contains("no argument `nope`; it takes pr")
        );
        assert!(
            built("pr_land", r#"return { yes = true }"#)
                .unwrap_err()
                .contains("--yes and --dry-run")
        );
        assert!(
            built("ci", r#"return { "--all" }"#)
                .unwrap_err()
                .contains("cannot start with `-`")
        );
        assert!(
            built("pr_threads", r#"return { "reply" }"#)
                .unwrap_err()
                .contains("subcommand")
        );
        assert!(
            built("ci", r#"return { "1", "2" }"#)
                .unwrap_err()
                .contains("too many")
        );
        assert!(
            built("ci", r#"return { all = "yes" }"#)
                .unwrap_err()
                .contains("true or false")
        );
    }

    #[test]
    fn command_lines_are_quoted_for_a_shell() {
        assert_eq!(quote("--body=done"), "--body=done");
        assert_eq!(quote("it's done"), r"'it'\''s done'");
        assert_eq!(quote(""), "''");
    }
}
