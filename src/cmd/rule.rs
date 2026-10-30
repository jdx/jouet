//! `jouet rule list|test|types|trust|untrust`.

use super::Output;
use crate::exit::Exit;
use crate::rules;
use crate::{RuleArgs, RuleCmd, RuleTestArgs};
use jouet_rules::{Call, Engine, Origin, Rule, Source, State, Trust};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

pub fn run(args: &RuleArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    match &args.cmd {
        RuleCmd::List(_) => list(&cwd, out),
        RuleCmd::Test(test_args) => test(test_args, &cwd, out),
        RuleCmd::Types(_) => {
            let text = jouet_rules::TYPES;
            out.result(&serde_json::json!({ "text": text }), || text.to_string());
            Exit::Ok
        }
        RuleCmd::Trust(_) => trust(&cwd, true, out),
        RuleCmd::Untrust(_) => trust(&cwd, false, out),
    }
}

#[derive(Serialize)]
struct ListedRule<'a> {
    name: &'a str,
    state: &'a State,
    mode: jouet_rules::Mode,
    scope: jouet_rules::Scope,
    author: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    created: &'a str,
    source: String,
    event: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    why: &'a str,
}

#[derive(Serialize)]
struct List<'a> {
    rules: Vec<ListedRule<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

fn list(cwd: &Path, out: &Output) -> Exit {
    let loaded = match rules::load(cwd) {
        Ok(loaded) => loaded,
        Err(e) => return out.error(Exit::Failed, &format!("starting the rule engine: {e}")),
    };
    let mut notes = loaded.errors.clone();
    if let Some((dir, trust)) = &loaded.repo {
        match trust {
            Trust::Trusted => {}
            Trust::Untrusted => notes.push(format!(
                "{} is not trusted, so its rules do not run; review them, then run `jouet rule trust`",
                dir.display()
            )),
            Trust::Changed => notes.push(format!(
                "{} changed since it was trusted, so its rules do not run; review them, then run `jouet rule trust`",
                dir.display()
            )),
        }
    }
    let listed: Vec<ListedRule> = loaded
        .engine
        .rules()
        .iter()
        .map(|rule| ListedRule {
            name: &rule.name,
            state: &rule.state,
            mode: rule.mode,
            scope: rule.scope,
            author: &rule.author,
            created: &rule.created,
            source: rule.origin.describe(),
            event: rule.event.name(),
            why: &rule.why,
        })
        .collect();
    let report = List {
        rules: listed,
        notes,
    };
    out.result(&report, || list_text(&report));
    Exit::Ok
}

fn state_text(state: &State) -> String {
    match state {
        State::Enabled => "enabled".into(),
        State::Disabled => "disabled".into(),
        State::Needs(command) => format!("needs {command}"),
        State::Untrusted => "untrusted".into(),
        State::Broken(error) => format!("broken: {error}"),
    }
}

fn list_text(report: &List) -> String {
    let rows: Vec<[String; 7]> = report
        .rules
        .iter()
        .map(|r| {
            // A post-tool-use rule records runs; its mode never applies.
            let mode = match (r.event, r.mode) {
                ("post-tool-use", _) => "record",
                (_, jouet_rules::Mode::Warn) => "warn",
                (_, jouet_rules::Mode::Deny) => "deny",
            };
            [
                r.name.to_string(),
                state_text(r.state),
                mode.to_string(),
                r.scope.name().to_string(),
                r.author.to_string(),
                r.created.to_string(),
                r.why.to_string(),
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..6)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in &rows {
        let mut line = String::new();
        for (cell, width) in row.iter().zip(&widths) {
            let _ = write!(line, "{cell:width$}  ");
        }
        line.push_str(&row[6]);
        let _ = writeln!(out, "{}", line.trim_end());
    }
    for note in &report.notes {
        let _ = writeln!(out, "note: {note}");
    }
    out
}

#[derive(Serialize)]
struct Tested {
    rule: String,
    event: &'static str,
    verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    records: Vec<jouet_rules::Record>,
}

#[derive(Serialize)]
struct Examples {
    rule: String,
    passed: usize,
    failed: usize,
    examples: Vec<jouet_rules::Example>,
}

fn test(args: &RuleTestArgs, cwd: &Path, out: &Output) -> Exit {
    let (engine, name) = match engine_for(&args.rule, cwd) {
        Ok(found) => found,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let Some(rule) = engine.rule(&name) else {
        return out.error(Exit::NotFound, &format!("no rule named `{}`", args.rule));
    };
    if let State::Broken(error) = &rule.state {
        return out.error(
            Exit::Failed,
            &format!("{} does not load: {error}", rule.name),
        );
    }
    let call = match (&args.payload, &args.command) {
        (Some(_), Some(_)) => {
            return out.error(Exit::Usage, "pass --payload or --command, not both");
        }
        (Some(path), None) => match read_payload(path) {
            Ok(payload) => Call::from_hook(&payload, "test"),
            Err(message) => return out.error(Exit::Usage, &message),
        },
        (None, Some(command)) => Call {
            tool: "Bash".into(),
            command: Some(command.clone()),
            harness: "test".into(),
            ..Call::default()
        },
        (None, None) => return examples(&engine, rule, out),
    };
    // The rule's own event decides how it runs; a post-tool-use rule with
    // no exit code given sees a command that succeeded.
    let call = Call {
        event: Some(rule.event),
        exit_code: match rule.event {
            jouet_rules::Event::PostToolUse => args.exit_code.or(call.exit_code).or(Some(0)),
            jouet_rules::Event::PreToolUse => None,
        },
        ..call
    };
    match engine.check(rule, &call) {
        Ok((decision, records)) => {
            let tested = Tested {
                rule: rule.name.clone(),
                event: rule.event.name(),
                verdict: jouet_rules::summarize(decision.as_ref(), &records),
                message: decision.and_then(|d| d.message),
                records,
            };
            out.result(&tested, || tested_text(&tested));
            Exit::Ok
        }
        Err(e) => out.error(Exit::Failed, &format!("rule {}: {e}", rule.name)),
    }
}

fn tested_text(t: &Tested) -> String {
    let mut out = match &t.message {
        Some(message) => format!("{}: {message}\n", t.verdict),
        None => format!("{}\n", t.verdict),
    };
    for record in &t.records {
        let _ = writeln!(
            out,
            "  {} `{}` exited {}",
            record.kind.name(),
            record.command,
            record.exit_code
        );
    }
    out
}

fn examples(engine: &Engine, rule: &Rule, out: &Output) -> Exit {
    let examples = match engine.examples(rule) {
        Ok(examples) => examples,
        Err(e) => return out.error(Exit::Failed, &format!("rule {}: {e}", rule.name)),
    };
    if examples.is_empty() {
        return out.error(
            Exit::Precondition,
            &format!(
                "{} has no examples; add some, or pass --command or --payload",
                rule.name
            ),
        );
    }
    let failed = examples.iter().filter(|e| !e.ok).count();
    let report = Examples {
        rule: rule.name.clone(),
        passed: examples.len() - failed,
        failed,
        examples,
    };
    out.result(&report, || {
        let mut text = String::new();
        for e in &report.examples {
            let mark = if e.ok { "ok  " } else { "FAIL" };
            let command = e.command.replace('\n', "\\n");
            let _ = write!(text, "{mark}  {:<12}  {command}", e.expected);
            if !e.ok {
                let _ = write!(text, "  (got {})", e.actual);
            }
            text.push('\n');
        }
        let _ = writeln!(
            text,
            "{}: {} of {} examples passed",
            report.rule,
            report.passed,
            report.examples.len()
        );
        text
    });
    if failed > 0 { Exit::Failed } else { Exit::Ok }
}

/// The engine to test with and the rule's name in it. A path loads that
/// file on its own; a name finds the rule among those installed, loading an
/// untrusted repository rule from its file, since testing only prints.
fn engine_for(rule: &str, cwd: &Path) -> Result<(Engine, String), (Exit, String)> {
    let as_path = Path::new(rule);
    let file = if rule.ends_with(".luau") || rule.contains(std::path::MAIN_SEPARATOR) {
        Some(as_path.to_path_buf())
    } else {
        None
    };
    if file.is_none() {
        let loaded = rules::load(cwd).map_err(|e| (Exit::Failed, e))?;
        let untrusted = loaded
            .engine
            .rules()
            .iter()
            .find(|r| r.name == rule && r.state == State::Untrusted)
            .map(|r| r.origin.clone());
        // Keep it a repository rule, so its scope and default mode are the
        // ones it will have once trusted.
        let Some(origin @ Origin::Repo(_)) = untrusted else {
            return Ok((loaded.engine, rule.to_string()));
        };
        return single(origin);
    }
    single(Origin::User(file.unwrap_or_else(|| as_path.to_path_buf())))
}

/// An engine holding just the rule at `origin`'s path, as that origin.
fn single(origin: Origin) -> Result<(Engine, String), (Exit, String)> {
    let (Origin::User(path) | Origin::Repo(path)) = &origin else {
        return Err((Exit::Failed, "a built-in rule has no file".into()));
    };
    let code = std::fs::read_to_string(path).map_err(|e| {
        let exit = if e.kind() == std::io::ErrorKind::NotFound {
            Exit::NotFound
        } else {
            Exit::Failed
        };
        (exit, format!("reading {}: {e}", path.display()))
    })?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let source = Source {
        name: name.clone(),
        origin,
        code,
        trusted: true,
    };
    let engine = Engine::new(
        vec![source],
        jouet_rules::Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    )
    .map_err(|e| (Exit::Failed, e.to_string()))?;
    Ok((engine, name))
}

fn read_payload(path: &str) -> Result<serde_json::Value, String> {
    let text = if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("reading the payload from stdin: {e}"))?;
        text
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?
    };
    serde_json::from_str(&text).map_err(|e| format!("the payload is not JSON: {e}"))
}

#[derive(Serialize)]
struct Trusted {
    dir: String,
    trusted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rules: Option<usize>,
}

fn trust(cwd: &Path, trust: bool, out: &Output) -> Exit {
    let Some(dir) = rules::repo_dir(cwd) else {
        return out.error(
            Exit::NotFound,
            "no .jouet/rules directory at the root of this repository",
        );
    };
    let mut store = rules::trust_store();
    let result = if trust {
        store.trust(&dir).map(|()| true)
    } else {
        store.untrust(&dir)
    };
    let changed = match result {
        Ok(changed) => changed,
        Err(e) => {
            return out.error(
                Exit::Failed,
                &format!("updating trust for {}: {e}", dir.display()),
            );
        }
    };
    let count = jouet_rules::rule_files(&dir).map(|f| f.len()).ok();
    let report = Trusted {
        dir: dir.display().to_string(),
        trusted: trust,
        rules: if trust { count } else { None },
    };
    out.result(&report, || match (trust, changed) {
        (true, _) => format!(
            "trusted {} ({} rules); a change to any of them needs trusting again\n",
            report.dir,
            count.unwrap_or(0)
        ),
        (false, true) => format!("no longer trusting {}\n", report.dir),
        (false, false) => format!("{} was not trusted\n", report.dir),
    });
    Exit::Ok
}
