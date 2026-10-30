//! A rule's examples: tool calls it must fire on, and near misses it must
//! leave alone, after ast-grep's valid and invalid rule tests.
//!
//! ```lua
//! examples = {
//!     fires = { "git stash", { command = "git stash pop", harness = "codex" } },
//!     quiet = { "git stash list", "echo 'git stash'" },
//! },
//! ```
//!
//! Every rule declares at least one of each. A rule of yours or a
//! repository's without them still runs, but only warns, and cannot be
//! promoted to deny or shared until it has them and passes them.

use crate::{Action, Call, Engine, Event, Kind, Record, Rule, State};
use mlua::{Table, Value};
use serde::Serialize;

/// The keys an example table may have.
const CASE_KEYS: &[&str] = &["command", "tool", "harness", "exit_code", "action", "kind"];

/// What a rule must make of an example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Deny, warn or record a check; `action` or `kind` pins which.
    Fires {
        action: Option<Action>,
        kind: Option<Kind>,
    },
    /// Say nothing, and record nothing.
    Quiet,
}

impl Expect {
    pub fn text(self) -> String {
        match self {
            Self::Fires {
                action: Some(Action::Deny),
                ..
            } => "fires deny".into(),
            Self::Fires {
                action: Some(Action::Warn),
                ..
            } => "fires warn".into(),
            Self::Fires {
                kind: Some(kind), ..
            } => format!("fires record {}", kind.name()),
            Self::Fires { .. } => "fires".into(),
            Self::Quiet => "quiet".into(),
        }
    }
}

/// One example: a tool call in compact form, and what the rule must make of
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub command: Option<String>,
    /// The harness's tool name; `Bash` unless the example says.
    pub tool: String,
    /// `test` unless the example names a harness, such as `codex`.
    pub harness: String,
    /// For a post-tool-use rule; 0 unless the example says.
    pub exit_code: Option<i32>,
    pub expect: Expect,
}

/// One example's result, as `jouet rule test` reports it.
#[derive(Debug, Serialize)]
pub struct Example {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub tool: String,
    pub harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// `fires`, `fires deny`, `fires record test` or `quiet`.
    pub expected: String,
    /// `deny`, `warn`, `record test`, `quiet` or `error: ...`.
    pub actual: String,
    /// What the rule told the agent, when it fired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub ok: bool,
    /// A `fires` example, rather than a `quiet` one.
    #[serde(skip)]
    pub fires: bool,
}

impl Example {
    /// The example as one line: its command, and its tool and harness when
    /// they are not the defaults.
    pub fn label(&self) -> String {
        let mut text = self.command.as_deref().unwrap_or("").replace('\n', "\\n");
        let mut extra = Vec::new();
        if self.tool != "Bash" {
            extra.push(format!("tool {}", self.tool));
        }
        if self.harness != "test" {
            extra.push(format!("harness {}", self.harness));
        }
        if !extra.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("({})", extra.join(", ")));
        }
        text
    }
}

/// Reads a rule table's `examples`: the cases it could read, and why they
/// do not cover both kinds, if they do not. Raw reads only, so none of the
/// rule's code runs.
pub(crate) fn read(table: &Table) -> (Vec<Case>, Option<String>) {
    match read_lists(table) {
        Ok((cases, problem)) => (cases, problem),
        Err(problem) => (Vec::new(), Some(problem)),
    }
}

fn read_lists(table: &Table) -> Result<(Vec<Case>, Option<String>), String> {
    let examples: Value = table.raw_get("examples").map_err(|e| e.to_string())?;
    let examples = match examples {
        Value::Nil => return Err("it has no examples".into()),
        Value::Table(t) => t,
        other => {
            return Err(format!(
                "`examples` must be a table of `fires` and `quiet` lists, not a {}",
                other.type_name()
            ));
        }
    };
    for pair in examples.pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|e| e.to_string())?;
        let key = key_name(&key);
        match key.as_str() {
            "fires" | "quiet" => {}
            "deny" | "warn" | "record" | "allow" => {
                return Err(format!(
                    "its examples are listed under `{key}`, which jouet no longer reads: list the calls it must fire on under `fires`, and the near misses it must leave alone under `quiet`"
                ));
            }
            _ => return Err(format!("`examples.{key}` is not `fires` or `quiet`")),
        }
    }
    let list = |name: &str| -> Result<Vec<Value>, String> {
        match examples.raw_get(name).map_err(|e| e.to_string())? {
            Value::Nil => Ok(Vec::new()),
            Value::Table(t) => t
                .sequence_values::<Value>()
                .collect::<mlua::Result<Vec<_>>>()
                .map_err(|e| format!("`examples.{name}`: {e}")),
            other => Err(format!(
                "`examples.{name}` must be a list, not a {}",
                other.type_name()
            )),
        }
    };
    let (fires, quiet) = (list("fires")?, list("quiet")?);
    let problem = match (fires.is_empty(), quiet.is_empty()) {
        (true, true) => return Err("it has no examples".into()),
        (true, false) => Some("it has no `fires` examples: calls it must fire on".to_string()),
        (false, true) => {
            Some("it has no `quiet` examples: near misses it must leave alone".to_string())
        }
        (false, false) => None,
    };
    let mut cases = Vec::new();
    for (name, items) in [("fires", fires), ("quiet", quiet)] {
        for (i, item) in items.into_iter().enumerate() {
            let at = format!("examples.{name}[{}]", i + 1);
            cases.push(case(item, name == "fires").map_err(|e| format!("`{at}`: {e}"))?);
        }
    }
    Ok((cases, problem))
}

fn key_name(key: &Value) -> String {
    match key {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(n) => n.to_string(),
        other => other.type_name().to_string(),
    }
}

fn case(item: Value, fires: bool) -> Result<Case, String> {
    let mut case = Case {
        command: None,
        tool: "Bash".into(),
        harness: "test".into(),
        exit_code: None,
        expect: if fires {
            Expect::Fires {
                action: None,
                kind: None,
            }
        } else {
            Expect::Quiet
        },
    };
    let t = match item {
        Value::String(s) => {
            case.command = Some(s.to_string_lossy());
            return Ok(case);
        }
        Value::Table(t) => t,
        other => {
            return Err(format!(
                "an example is a command or a table, not a {}",
                other.type_name()
            ));
        }
    };
    for pair in t.pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|e| e.to_string())?;
        let key = key_name(&key);
        if !CASE_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "unknown key `{key}`; an example has {}",
                CASE_KEYS.join(", ")
            ));
        }
    }
    let text = |key: &str| {
        t.raw_get::<Option<String>>(key)
            .map_err(|e| format!("`{key}`: {e}"))
    };
    case.command = text("command")?;
    if let Some(tool) = text("tool")? {
        case.tool = tool;
    }
    if let Some(harness) = text("harness")? {
        case.harness = harness;
    }
    if case.command.is_none() && case.tool == "Bash" {
        return Err("a shell example needs a `command`".into());
    }
    case.exit_code = t
        .raw_get::<Option<i32>>("exit_code")
        .map_err(|e| format!("`exit_code`: {e}"))?;
    let action = match text("action")?.as_deref() {
        None => None,
        Some("deny") => Some(Action::Deny),
        Some("warn") => Some(Action::Warn),
        Some(other) => {
            return Err(format!(
                "`action` must be \"deny\" or \"warn\", not `{other}`"
            ));
        }
    };
    let kind = match text("kind")? {
        None => None,
        Some(kind) => Some(Kind::parse(&kind).ok_or_else(|| {
            format!("`kind` must be \"test\", \"lint\" or \"build\", not `{kind}`")
        })?),
    };
    if !fires && (action.is_some() || kind.is_some()) {
        return Err("a `quiet` example takes no `action` or `kind`".into());
    }
    if fires {
        case.expect = Expect::Fires { action, kind };
    }
    Ok(case)
}

/// Whether what the rule did is what the example expects.
fn met(expect: Expect, action: Option<Action>, records: &[Record]) -> bool {
    let spoke = matches!(action, Some(Action::Deny | Action::Warn));
    match expect {
        Expect::Quiet => !spoke && records.is_empty(),
        Expect::Fires { action: want, kind } => {
            (spoke || !records.is_empty())
                && want.is_none_or(|want| action == Some(want))
                && kind.is_none_or(|kind| records.iter().any(|r| r.kind == kind))
        }
    }
}

impl Engine {
    /// Runs a rule's examples and compares each with what it expects. The
    /// rule's mode does not apply: a warn-mode rule's example that denies
    /// shows `deny`. A rule whose examples could not be read has none to run;
    /// [`Rule::missing_examples`] says why.
    pub fn examples(&self, rule: &Rule) -> Result<Vec<Example>, String> {
        if rule.table.is_none() {
            return Err(match &rule.state {
                State::Broken(e) => e.clone(),
                _ => "the rule is not loaded".into(),
            });
        }
        let mut out = Vec::new();
        for case in &rule.examples {
            let call = Call {
                event: Some(rule.event),
                harness: case.harness.clone(),
                tool: case.tool.clone(),
                command: case.command.clone(),
                cwd: None,
                exit_code: match rule.event {
                    Event::PostToolUse => Some(case.exit_code.unwrap_or(0)),
                    Event::PreToolUse => case.exit_code,
                },
            };
            // Each example is a call of its own: probes are asked afresh.
            self.vm.new_call();
            let (ok, actual, message) = match self.run_check(rule, &call, false) {
                Ok((decision, records)) => {
                    let action = decision.as_ref().map(|d| d.action);
                    let actual = match crate::summarize(decision.as_ref(), &records) {
                        word if word == "allow" => "quiet".to_string(),
                        word => word,
                    };
                    let message = decision
                        .filter(|d| d.action != Action::Allow)
                        .and_then(|d| d.message);
                    (met(case.expect, action, &records), actual, message)
                }
                Err(e) => (false, format!("error: {e}"), None),
            };
            out.push(Example {
                command: case.command.clone(),
                tool: case.tool.clone(),
                harness: case.harness.clone(),
                exit_code: call.exit_code,
                expected: case.expect.text(),
                actual,
                message,
                ok,
                fires: matches!(case.expect, Expect::Fires { .. }),
            });
        }
        Ok(out)
    }

    /// Whether a rule has examples of both kinds and passes every one: what
    /// a rule of yours needs before it may deny, or be shared. Returns how
    /// many it passed, or why it falls short, as a clause such as `it has no
    /// examples`.
    pub fn proven(&self, rule: &Rule) -> Result<usize, String> {
        if let Some(problem) = &rule.missing_examples {
            return Err(problem.clone());
        }
        let examples = self.examples(rule)?;
        let failed = examples.iter().filter(|e| !e.ok).count();
        if failed > 0 {
            return Err(format!("{failed} of its {} examples fail", examples.len()));
        }
        Ok(examples.len())
    }
}
