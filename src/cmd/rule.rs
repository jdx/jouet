//! `jouet rule list|test|schema|types|trust|untrust|new|promote|log`.

use super::Output;
use crate::rules;
use crate::wind::{self, Paths, Status, replay, rulefile};
use crate::{
    RuleArgs, RuleCmd, RuleGapArgs, RuleLogArgs, RuleNewArgs, RulePromoteArgs, RuleTestArgs,
};
use jouet_output::exit::Exit;
use jouet_rules::{Call, Engine, Origin, Rule, Source, State, Trust};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

pub(crate) fn run(args: &RuleArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    match &args.cmd {
        RuleCmd::List(_) => list(&cwd, out),
        RuleCmd::Test(test_args) => test(test_args, &cwd, out),
        RuleCmd::Schema(_) => {
            let text = jouet_rules::schema();
            let schema: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            out.result(&schema, || text.clone());
            Exit::Ok
        }
        RuleCmd::Types(_) => {
            let text = jouet_rules::TYPES;
            out.result(&serde_json::json!({ "text": text }), || text.to_string());
            Exit::Ok
        }
        RuleCmd::Trust(_) => trust(&cwd, true, out),
        RuleCmd::Untrust(_) => trust(&cwd, false, out),
        RuleCmd::New(new_args) => new(new_args, out),
        RuleCmd::Promote(promote_args) => promote(promote_args, out),
        RuleCmd::Log(log_args) => log(log_args, out),
        RuleCmd::Gap(gap_args) => gap(gap_args, out),
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
    /// `toml`, or `luau` for a deprecated Luau rule file.
    format: jouet_rules::Format,
    /// The Luau script a TOML rule names.
    #[serde(skip_serializing_if = "Option::is_none")]
    script: Option<&'a str>,
    /// Why its examples fall short, so that it only warns.
    #[serde(skip_serializing_if = "Option::is_none")]
    missing_examples: Option<&'a str>,
}

#[derive(Serialize)]
struct List<'a> {
    rules: Vec<ListedRule<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

fn list(cwd: &Path, out: &Output) -> Exit {
    let mut loaded = rules::load(cwd);
    // Listing is not on a hook's path, so it compiles every script to show
    // the broken ones.
    loaded.engine.load_scripts();
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
            format: rule.format,
            script: rule.script.as_deref(),
            missing_examples: rule.missing_examples.as_deref(),
        })
        .collect();
    notes.extend(
        loaded
            .engine
            .rules()
            .iter()
            .filter_map(jouet_rules::Rule::deprecation),
    );
    // Listing is not on a hook's path, so it can afford to run examples.
    for rule in loaded.engine.rules() {
        if rule.scope == jouet_rules::Scope::Builtin
            || matches!(rule.state, State::Broken(_) | State::Untrusted)
        {
            continue;
        }
        let name = &rule.name;
        match (&rule.missing_examples, loaded.engine.proven(rule)) {
            (Some(missing), _) => notes.push(format!(
                "{name} only warns, and cannot be promoted, until it has examples of both kinds: {missing}; see `jouet rule test {name}`"
            )),
            (None, Err(failing)) => notes.push(format!(
                "{name}: {failing}; see `jouet rule test {name}`"
            )),
            (None, Ok(_)) => {}
        }
    }
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

/// One rule's examples and how they went.
#[derive(Serialize)]
struct Examples {
    rule: String,
    passed: usize,
    failed: usize,
    /// Why the examples do not cover both kinds, or could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    missing: Option<String>,
    /// Why the rule could not run at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    examples: Vec<jouet_rules::Example>,
}

impl Examples {
    fn run(engine: &Engine, rule: &Rule) -> Self {
        let (examples, error) = match &rule.state {
            State::Broken(e) => (Vec::new(), Some(format!("it does not load: {e}"))),
            _ => match engine.examples(rule) {
                Ok(examples) => (examples, None),
                Err(e) => (Vec::new(), Some(e)),
            },
        };
        crate::share::record_gaps(engine);
        let failed = examples.iter().filter(|e| !e.ok).count();
        Self {
            rule: rule.name.clone(),
            passed: examples.len() - failed,
            failed,
            missing: rule.missing_examples.clone(),
            error,
            examples,
        }
    }

    fn ok(&self) -> bool {
        self.failed == 0 && self.missing.is_none() && self.error.is_none()
    }

    /// Each example on a line (or only those that failed), and under one
    /// that failed, what it expected and what the rule did, as a diff.
    fn lines(&self, indent: &str, failed_only: bool) -> String {
        let mut text = String::new();
        for e in self.examples.iter().filter(|e| !(failed_only && e.ok)) {
            let mark = if e.ok { "ok  " } else { "FAIL" };
            let _ = writeln!(text, "{indent}{mark}  {:<12}  {}", e.expected, e.label());
            if !e.ok {
                let actual = match &e.message {
                    Some(message) => format!("{}: {message}", e.actual),
                    None => e.actual.clone(),
                };
                let _ = writeln!(text, "{indent}      - expected: {}", e.expected);
                let _ = writeln!(text, "{indent}      + actual:   {actual}");
            }
        }
        text
    }
}

/// How to give a rule what it lacks.
const ADD_EXAMPLES: &str = "give it `[rule.examples]` with `fires = [...]` and `quiet = [...]`";

fn test(args: &RuleTestArgs, cwd: &Path, out: &Output) -> Exit {
    let name = match (&args.rule, args.all) {
        (Some(_), true) => return out.error(Exit::Usage, "pass a rule or --all, not both"),
        (None, false) => return out.error(Exit::Usage, "name a rule to test, or pass --all"),
        (None, true) => {
            if args.command.is_some() || args.payload.is_some() || args.replay {
                return out.error(
                    Exit::Usage,
                    "--all runs every rule's examples; leave out --command, --payload and --replay",
                );
            }
            return test_all(cwd, out);
        }
        (Some(name), false) => name,
    };
    let (engine, names) = match engine_for(name, cwd) {
        Ok(found) => found,
        Err((exit, message)) => return out.error(exit, &message),
    };
    for rule in engine.rules() {
        if let Some(note) = rule.deprecation()
            && names.contains(&rule.name)
        {
            eprintln!("jouet: {note}");
        }
    }
    let name = match names.as_slice() {
        [name] => name.clone(),
        _ if args.command.is_some() || args.payload.is_some() || args.replay => {
            return out.error(
                Exit::Usage,
                &format!(
                    "{name} holds the rules {}; to test one against a command, payload or the ledger, install the file and name the rule",
                    names.join(", ")
                ),
            );
        }
        _ => return report_examples(&engine, &names, out),
    };
    let Some(rule) = engine.rule(&name) else {
        return out.error(Exit::NotFound, &format!("no rule named `{name}`"));
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
        (None, Some(command)) => {
            let cwd = std::env::current_dir().ok();
            Call {
                tool: "Bash".into(),
                command: Some(command.clone()),
                harness: "test".into(),
                fnox: cwd.as_deref().and_then(crate::fnox::context),
                cwd: cwd.map(|d| d.to_string_lossy().into_owned()),
                ..Call::default()
            }
        }
        (None, None) if args.replay => return replay_ledger(&engine, rule, out),
        (None, None) => return examples(&engine, rule, out),
    };
    if args.replay {
        return out.error(
            Exit::Usage,
            "--replay runs over the ledger; leave out --command and --payload",
        );
    }
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
    let checked = engine.check(rule, &call);
    crate::share::record_gaps(&engine);
    match checked {
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
    let report = Examples::run(engine, rule);
    if let Some(error) = &report.error {
        return out.error(Exit::Failed, &format!("rule {}: {error}", rule.name));
    }
    if let (true, Some(missing)) = (report.examples.is_empty(), &report.missing) {
        return out.error(
            Exit::Precondition,
            &format!(
                "{}: {missing}; {ADD_EXAMPLES}. Or pass --command or --payload",
                rule.name
            ),
        );
    }
    out.result(&report, || {
        let mut text = report.lines("", false);
        let _ = writeln!(
            text,
            "{}: {} of {} examples passed",
            report.rule,
            report.passed,
            report.examples.len()
        );
        if let Some(missing) = &report.missing {
            let _ = writeln!(text, "{}: {missing}; {ADD_EXAMPLES}", report.rule);
        }
        text
    });
    if report.ok() { Exit::Ok } else { Exit::Failed }
}

#[derive(Serialize)]
struct All {
    passed: usize,
    failed: usize,
    rules: Vec<Examples>,
}

/// `rule test --all`: every rule's examples, including a repository's
/// untrusted ones, since testing only prints.
fn test_all(cwd: &Path, out: &Output) -> Exit {
    let mut loaded = rules::load(cwd);
    loaded.engine.load_scripts();
    let mut reports = Vec::new();
    for rule in loaded.engine.rules() {
        let report = match (&rule.state, &rule.origin) {
            (State::Untrusted, origin @ Origin::Repo(_)) => match single(origin.clone(), cwd) {
                Ok((engine, _)) => match engine.rule(&rule.name) {
                    Some(rule) => Examples::run(&engine, rule),
                    None => continue,
                },
                Err((_, error)) => Examples {
                    rule: rule.name.clone(),
                    passed: 0,
                    failed: 0,
                    missing: None,
                    error: Some(error),
                    examples: Vec::new(),
                },
            },
            _ => Examples::run(&loaded.engine, rule),
        };
        reports.push(report);
    }
    print_reports(reports, out)
}

/// The examples of the named rules in `engine`, as `rule test --all` shows
/// them: for a rule file holding several.
fn report_examples(engine: &Engine, names: &[String], out: &Output) -> Exit {
    let reports = engine
        .rules()
        .iter()
        .filter(|r| names.contains(&r.name))
        .map(|rule| Examples::run(engine, rule))
        .collect();
    print_reports(reports, out)
}

fn print_reports(reports: Vec<Examples>, out: &Output) -> Exit {
    let failed = reports.iter().filter(|r| !r.ok()).count();
    let report = All {
        passed: reports.len() - failed,
        failed,
        rules: reports,
    };
    out.result(&report, || {
        let width = report
            .rules
            .iter()
            .map(|r| r.rule.chars().count())
            .max()
            .unwrap_or(0);
        let mut text = String::new();
        for r in &report.rules {
            let mut problems = Vec::new();
            if let Some(error) = &r.error {
                problems.push(error.clone());
            }
            if r.failed > 0 {
                problems.push(format!(
                    "{} of {} examples failed",
                    r.failed,
                    r.examples.len()
                ));
            }
            if let Some(missing) = &r.missing {
                problems.push(missing.clone());
            }
            let (mark, detail) = if problems.is_empty() {
                ("ok  ", format!("{} examples passed", r.passed))
            } else {
                ("FAIL", problems.join("; "))
            };
            let _ = writeln!(text, "{mark}  {:width$}  {detail}", r.rule);
            text.push_str(&r.lines("      ", true));
        }
        let _ = writeln!(
            text,
            "{} of {} rules passed their examples",
            report.passed,
            report.rules.len()
        );
        text
    });
    if failed > 0 { Exit::Failed } else { Exit::Ok }
}

#[derive(Serialize)]
struct Replayed {
    rule: String,
    #[serde(flatten)]
    report: replay::Replay,
}

fn replay_ledger(engine: &Engine, rule: &Rule, out: &Output) -> Exit {
    let Some(dir) = crate::ledger::dir() else {
        return out.error(Exit::Failed, "there is no home directory, so no ledger");
    };
    let entries = wind::reader::read_all(&dir);
    if entries.is_empty() {
        return out.error(
            Exit::Precondition,
            &format!(
                "the ledger in {} is empty; it fills as jouet's hooks run (`jouet setup`)",
                dir.display()
            ),
        );
    }
    let report = Replayed {
        rule: rule.name.clone(),
        report: replay::replay(engine, rule, &entries),
    };
    crate::share::record_gaps(engine);
    out.result(&report, || replayed_text(&report));
    if report.report.fired_total == 0 {
        Exit::Failed
    } else {
        Exit::Ok
    }
}

fn replayed_text(r: &Replayed) -> String {
    let rep = &r.report;
    let mut text = format!(
        "{}: fired on {} of {} calls in the ledger\n",
        r.rule, rep.fired_total, rep.calls
    );
    let section = |text: &mut String, title: &str, hits: &[replay::Hit], distinct: usize| {
        if hits.is_empty() {
            return;
        }
        let _ = writeln!(text, "{title}:");
        for hit in hits {
            let _ = writeln!(
                text,
                "  {:>4}x  {:<11}  {}",
                hit.count, hit.verdict, hit.command
            );
        }
        if distinct > hits.len() {
            let _ = writeln!(text, "  ... {} more", distinct - hits.len());
        }
    };
    section(&mut text, "fires on", &rep.fired, rep.fired_distinct);
    section(
        &mut text,
        "stays quiet on (near misses)",
        &rep.near_misses,
        rep.near_misses_distinct,
    );
    for e in &rep.errors {
        let _ = writeln!(text, "error: {e}");
    }
    if rep.fired_total == 0 {
        text.push_str("it fired on nothing the hooks have seen: check what it matches\n");
    }
    text
}

/// Who is writing a rule: the flag, else the session from the environment.
fn author(flag: Option<&str>) -> String {
    flag.map(str::to_string)
        .or_else(|| {
            ["JOUET_SESSION_ID", "CLAUDE_SESSION_ID", "CODEX_THREAD_ID"]
                .iter()
                .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        })
        .unwrap_or_else(|| "user".into())
}

#[derive(Serialize)]
struct Created {
    rule: String,
    path: String,
    /// Its script, for a rule with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    script: Option<String>,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_note: Option<String>,
}

fn new(args: &RuleNewArgs, out: &Output) -> Exit {
    let name = &args.name;
    if !rulefile::valid_name(name) {
        return out.error(
            Exit::Usage,
            &format!(
                "`{name}` is not a rule name: use lowercase words joined by dashes, such as no-tags"
            ),
        );
    }
    if jouet_rules::builtin().iter().any(|s| &s.name == name) {
        return out.error(
            Exit::Precondition,
            &format!("`{name}` is a built-in rule, which cannot be replaced; choose another name"),
        );
    }
    let Some(paths) = Paths::locate() else {
        return out.error(Exit::Failed, "there is no home directory to keep rules in");
    };
    let path = paths.rules.join(format!("{name}.toml"));
    let script_path = paths.rules.join(format!("{name}.luau"));
    if let Some(taken) = [&path, &script_path].into_iter().find(|p| p.exists()) {
        return out.error(
            Exit::Precondition,
            &format!(
                "{} already exists; edit it, or choose another name",
                taken.display()
            ),
        );
    }
    let author = author(args.author.as_deref());
    let created = jouet_output::time::date(crate::wind::now());
    let (code, script, reason) = match &args.from_note {
        Some(id) => {
            let state = wind::State::load(&paths.state);
            let Some(note) = state.note(id) else {
                return out.error(Exit::NotFound, &format!("no winding note `{id}`"));
            };
            match adopt(note, name, &author, args.why.as_deref()) {
                Ok(draft) => (
                    draft.code,
                    draft.script,
                    format!("from winding note {id}: {}", note.why),
                ),
                Err(problem) => {
                    return out.error(
                        Exit::Precondition,
                        &format!(
                            "note {id}'s draft does not pass examples of both kinds ({problem}); `jouet wind dismiss {id}`, or write the rule with `jouet rule new {name} --why ...`"
                        ),
                    );
                }
            }
        }
        None => {
            let Some(why) = &args.why else {
                return out.error(
                    Exit::Usage,
                    "pass --why with what the rule is for, or --from-note with a winding note",
                );
            };
            let header = rulefile::Header {
                name,
                why,
                author: &author,
                created: &created,
            };
            let (code, script) = rulefile::scaffold(&header, args.script);
            (code, script, why.clone())
        }
    };
    let write = |path: &Path, text: &str| {
        std::fs::create_dir_all(&paths.rules).and_then(|()| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()))
        })
    };
    let written = match &script {
        Some(script) => write(&script_path, script).map(|()| Some(&script_path)),
        None => Ok(None),
    }
    .and_then(|script| write(&path, &code).map(|()| script));
    let script_written = match written {
        Ok(script) => script,
        Err(e) => return out.error(Exit::Failed, &format!("writing {}: {e}", path.display())),
    };
    let change = rulefile::Change {
        ts: crate::wind::now(),
        rule: name.clone(),
        change: "created".into(),
        from: None,
        to: Some("warn".into()),
        reason,
        by: author,
    };
    if let Err(e) = rulefile::log(&paths.log, &change) {
        eprintln!("jouet: writing the rule log: {e}");
    }
    if let Some(id) = &args.from_note {
        wind::set_status(&paths, id, Status::Adopted);
    }
    let report = Created {
        rule: name.clone(),
        path: path.display().to_string(),
        script: script_written.map(|p| p.display().to_string()),
        mode: "warn",
        from_note: args.from_note.clone(),
    };
    out.result(&report, || {
        let next = if args.from_note.is_some() {
            format!("check it: `jouet rule test {name}` and `jouet rule test {name} --replay`")
        } else if report.script.is_some() {
            format!(
                "fill in its examples (calls it must fire on, near misses it must leave alone), its matcher and its script's check, then `jouet rule test {name}`"
            )
        } else {
            format!(
                "fill in its examples (calls it must fire on, near misses it must leave alone), its matcher and its message, then `jouet rule test {name}`"
            )
        };
        let files = match &report.script {
            Some(script) => format!("{} and {script}", report.path),
            None => report.path.clone(),
        };
        format!(
            "wrote {files} (warn mode); {next}; `jouet rule promote {name}` when it is right\n"
        )
    });
    Exit::Ok
}

/// A winding note's draft as the rule `name`, written by `author`: its
/// name, author, `why` and script path set, and checked to load in warn
/// mode and pass examples of both kinds.
fn adopt(
    note: &wind::Note,
    name: &str,
    author: &str,
    why: Option<&str>,
) -> Result<wind::draft::Draft, String> {
    // A jouet from before rules were TOML drafted a Luau file.
    if note.code.trim_start().starts_with("--") {
        return Err("it was drafted as a Luau rule file, which jouet no longer writes".into());
    }
    let mut code = rulefile::set_field(&note.code, &note.rule, "author", author)?;
    if let Some(why) = why {
        code = rulefile::set_field(&code, &note.rule, "why", why)?;
    }
    if note.script.is_some() {
        code = rulefile::set_field(&code, &note.rule, "script", &format!("{name}.luau"))?;
    }
    let code = rulefile::set_field(&code, &note.rule, "name", name)?;
    let draft = wind::draft::Draft {
        name: name.to_string(),
        why: why.unwrap_or(&note.why).to_string(),
        message: note.message.clone(),
        code,
        script: note.script.clone(),
    };
    wind::draft::verify(&draft)?;
    Ok(draft)
}

fn promote(args: &RulePromoteArgs, out: &Output) -> Exit {
    let Some(paths) = Paths::locate() else {
        return out.error(Exit::Failed, "there is no home directory to keep rules in");
    };
    let reason = args
        .reason
        .clone()
        .unwrap_or_else(|| "promoted by hand".into());
    match wind::promote(&paths, &args.name, "user", &reason, crate::wind::now()) {
        Ok(change) => {
            out.result(&change, || {
                format!("{} now denies (was warn)\n", change.rule)
            });
            Exit::Ok
        }
        Err(message) if message.starts_with("no rule") => out.error(Exit::NotFound, &message),
        Err(message) => out.error(Exit::Precondition, &message),
    }
}

#[derive(Serialize)]
struct Log {
    changes: Vec<rulefile::Change>,
}

fn log(args: &RuleLogArgs, out: &Output) -> Exit {
    let changes: Vec<rulefile::Change> = rulefile::log_path()
        .map(|p| rulefile::read_log(&p))
        .unwrap_or_default()
        .into_iter()
        .filter(|c| args.name.as_ref().is_none_or(|n| *n == c.rule))
        .collect();
    let report = Log { changes };
    out.result(&report, || {
        if report.changes.is_empty() {
            return "no changes logged\n".into();
        }
        let mut text = String::new();
        for c in &report.changes {
            let arrow = match (&c.from, &c.to) {
                (Some(from), Some(to)) if c.change == "promoted" => format!(" {from} -> {to}"),
                _ => String::new(),
            };
            let _ = writeln!(
                text,
                "{}  {}  {}{arrow}  by {}: {}",
                jouet_output::time::date(c.ts),
                c.rule,
                c.change,
                c.by,
                c.reason
            );
        }
        text
    });
    Exit::Ok
}

fn gap(args: &RuleGapArgs, out: &Output) -> Exit {
    if args.text.trim().is_empty() {
        return out.error(Exit::Usage, "say what the rule could not express");
    }
    let Some(path) = crate::share::gaps_path() else {
        return out.error(Exit::Failed, "there is no home directory to record gaps in");
    };
    if let Err(e) = crate::share::record_note(&path, &args.text, args.rule.as_deref(), wind::now())
    {
        return out.error(Exit::Failed, &format!("recording the gap: {e}"));
    }
    out.result(&serde_json::json!({ "recorded": args.text.trim() }), || {
        "recorded; `jouet share --idea` turns recorded gaps into a proposal for jouet's maintainers\n".into()
    });
    Exit::Ok
}

/// The engine to test with and the names of the rules to test in it. A
/// path loads that file on its own (a TOML file may hold several rules); a
/// name finds the rule among those installed, loading an untrusted
/// repository rule from its file, since testing only prints.
fn engine_for(rule: &str, cwd: &Path) -> Result<(Engine, Vec<String>), (Exit, String)> {
    let as_path = Path::new(rule);
    let is_file = rule.ends_with(".toml")
        || rule.ends_with(".luau")
        || rule.contains(std::path::MAIN_SEPARATOR);
    if is_file {
        return single(Origin::User(as_path.to_path_buf()), cwd);
    }
    let mut loaded = rules::load(cwd);
    loaded.engine.load_scripts();
    let untrusted = loaded
        .engine
        .rules()
        .iter()
        .find(|r| r.name == rule && r.state == State::Untrusted)
        .map(|r| r.origin.clone());
    // Keep it a repository rule, so its scope and default mode are the
    // ones it will have once trusted.
    let Some(origin @ Origin::Repo(_)) = untrusted else {
        return Ok((loaded.engine, vec![rule.to_string()]));
    };
    let (engine, _) = single(origin, cwd)?;
    Ok((engine, vec![rule.to_string()]))
}

/// An engine holding just the rules of the file at `origin`'s path, as
/// that origin, with the probes a hook in `cwd` would have, and their names.
fn single(origin: Origin, cwd: &Path) -> Result<(Engine, Vec<String>), (Exit, String)> {
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
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let source = if path.extension().is_some_and(|e| e == "luau") {
        Source::luau(stem, origin.clone(), code)
    } else {
        Source::toml(stem, origin.clone(), code)
    };
    let mut engine = Engine::new(
        vec![source],
        jouet_rules::Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    );
    engine.load_scripts();
    let network = crate::settings::Settings::load().rules.network;
    engine.set_probes(rules::probes(cwd, network));
    let names = engine.rules().iter().map(|r| r.name.clone()).collect();
    Ok((engine, names))
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
    // The rules the directory holds, scripts aside.
    let count = trust.then(|| {
        rules::load(cwd)
            .engine
            .rules()
            .iter()
            .filter(|r| matches!(&r.origin, Origin::Repo(path) if path.starts_with(&dir)))
            .count()
    });
    let report = Trusted {
        dir: dir.display().to_string(),
        trusted: trust,
        rules: count,
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
