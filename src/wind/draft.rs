//! Drafting a rule from a noticed pattern: a Luau file against the public
//! `jouet` API, in warn mode, whose `fires` examples are the calls that
//! motivated it and whose `quiet` examples are the near misses it must leave
//! alone: what worked instead, and calls from the ledger that run the same
//! programs.

use super::detect::{Evidence, Pattern};
use super::reader;
use super::replay;
use super::rulefile::{self, Header};
use crate::ledger::Entry;
use std::fmt::Write as _;

/// Near misses from the ledger a draft takes as `quiet` examples, at most.
const MAX_NEAR_MISSES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    pub name: String,
    pub why: String,
    /// What the rule tells the agent.
    pub message: String,
    pub code: String,
}

/// A Luau string literal.
pub(crate) fn lua_str(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| lua_str(i)).collect();
    format!("{{ {} }}", quoted.join(", "))
}

/// A rule name from a pattern and the command's words: `wind-gh-pr-view`.
pub(crate) fn name_for(evidence: &Evidence) -> String {
    let mut words: Vec<String> = Vec::new();
    for word in &evidence.signature.words {
        let slug: String = word
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let slug = slug.trim_matches('-').to_string();
        if !slug.is_empty() {
            words.push(slug);
        }
    }
    let mut name = format!("wind-{}", words.join("-"));
    if evidence.pattern == Pattern::NeverWorks {
        name.push_str("-fails");
    }
    name.truncate(56);
    name.trim_end_matches('-').to_string()
}

/// What a pattern cost and what to do about it, in one line each.
fn describe(e: &Evidence) -> (String, String) {
    // The first shape seen reads better than the signature: `mytool <arg>
    // --all` rather than `mytool --all`.
    let command = e
        .examples
        .first()
        .cloned()
        .unwrap_or_else(|| e.signature.text());
    let sessions = e.sessions.len();
    match e.pattern {
        Pattern::NeverWorks => {
            let code = e.exit_code.unwrap_or(1);
            let how = match code {
                127 => ", not found",
                126 => ", not executable",
                _ => "",
            };
            let why = format!(
                "`{command}` failed {} times in {sessions} sessions (last exit {code}{how}) and never worked.",
                e.count
            );
            let message = match &e.fix {
                Some(fix) => format!(
                    "`{command}` has failed every time it ran here (exit {code}{how}); `{fix}` worked instead."
                ),
                None => format!(
                    "`{command}` has failed every time it ran here (exit {code}{how}); find out why before running it again."
                ),
            };
            (why, message)
        }
        Pattern::Sleep => (
            format!(
                "Agents spent {} waiting in {} long sleeps across {sessions} sessions.",
                jouet_output::time::elapsed(e.cost / 1000),
                e.count
            ),
            "Instead of sleeping, wait for what you need: `jouet wait` blocks until a file, log line, port, URL, process or command is ready (`jouet agent-help wait`).".to_string(),
        ),
    }
}

/// The rule for a pattern, named `name`, as `jouet wind` writes it. `near`
/// holds ledger calls it leaves alone, from [`near_misses`], which join its
/// `quiet` examples.
pub(crate) fn draft(e: &Evidence, name: &str, created: &str, near: &[String]) -> Draft {
    let (why, message) = describe(e);
    let header = rulefile::header(&Header {
        name,
        why: &why,
        author: "jouet wind",
        created,
    });
    // Examples are commands: the ledger's placeholders become plain words.
    let examples: Vec<String> = e.examples.iter().map(|x| reader::script(x)).collect();
    let (body, mut quiet) = match e.pattern {
        Pattern::NeverWorks => {
            let mut quiet: Vec<String> = e.fix.iter().map(|f| reader::script(f)).collect();
            quiet.push(format!("echo {}", e.signature.words.join(" ")));
            // Without one of its flags, the command is a near miss.
            if let Some(flag) = e.signature.flags.first() {
                let fewer: Vec<&str> = e
                    .signature
                    .words
                    .iter()
                    .map(String::as_str)
                    .chain(
                        e.signature
                            .flags
                            .iter()
                            .map(String::as_str)
                            .filter(|f| f != flag),
                    )
                    .collect();
                quiet.push(fewer.join(" "));
            }
            (
                format!(
                    "local WORDS = {}\nlocal FLAGS = {}\nlocal MESSAGE = {}\n{MATCHES}",
                    list(&e.signature.words),
                    list(&e.signature.flags),
                    lua_str(&message)
                ),
                quiet,
            )
        }
        Pattern::Sleep => (
            format!("local MESSAGE = {}\n{SLEEPS}", lua_str(&message)),
            vec!["sleep 1".to_string(), "echo sleep 60".to_string()],
        ),
    };
    for shape in near {
        let script = reader::script(shape);
        if !quiet.contains(&script) && !examples.contains(&script) {
            quiet.push(script);
        }
    }
    let needs = if e.pattern == Pattern::Sleep {
        "\tneeds = { \"wait\" },\n"
    } else {
        ""
    };
    let code = format!(
        "{header}\n-- Drafted by `jouet wind` from this machine's hook ledger. Check it with\n-- `jouet rule test {name} --replay` before relying on it.\n\n{body}\nlocal rule: JouetRule = {{\n{needs}\tcheck = function(call: JouetCall): JouetDecision?\n\t\tfor _, command in call.commands do\n\t\t\tif not command.nested and matches(command) then\n\t\t\t\treturn jouet.deny(MESSAGE)\n\t\t\tend\n\t\tend\n\t\treturn nil\n\tend,\n\texamples = {{\n\t\t-- The calls that showed the pattern.\n\t\tfires = {examples},\n\t\t-- What worked instead, and calls running the same programs.\n\t\tquiet = {quiet},\n\t}},\n}}\n\nreturn rule\n",
        examples = list(&examples),
        quiet = list(&quiet),
    );
    Draft {
        name: name.to_string(),
        why,
        message,
        code,
    }
}

/// A draft's rule, loaded on its own as a rule of yours.
fn load(name: &str, code: &str) -> Option<jouet_rules::Engine> {
    jouet_rules::Engine::new(
        vec![jouet_rules::Source {
            name: name.to_string(),
            origin: jouet_rules::Origin::User(format!("{name}.luau").into()),
            code: code.to_string(),
            trusted: true,
        }],
        jouet_rules::Options {
            has_command: Box::new(|_| true),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    )
    .ok()
}

/// The calls in the ledger that run the pattern's program and that a draft
/// leaves alone, most frequent first: near misses for its `quiet` examples.
pub(crate) fn near_misses(draft: &Draft, e: &Evidence, entries: &[Entry]) -> Vec<String> {
    let Some(program) = e.signature.words.first() else {
        return Vec::new();
    };
    let Some(engine) = load(&draft.name, &draft.code) else {
        return Vec::new();
    };
    let Some(rule) = engine.rule(&draft.name) else {
        return Vec::new();
    };
    replay::replay(&engine, rule, entries)
        .near_misses
        .into_iter()
        .filter(|hit| replay::programs(&hit.command).contains(program))
        .take(MAX_NEAR_MISSES)
        .map(|hit| hit.command)
        .collect()
}

/// Checks that a drafted rule loads as a warn-mode rule of yours and passes
/// examples of both kinds, returning how many, or what is wrong.
pub(crate) fn verify(name: &str, code: &str) -> Result<usize, String> {
    let engine = load(name, code).ok_or("the rule engine did not start")?;
    let rule = engine.rule(name).ok_or("the rule did not load")?;
    if let jouet_rules::State::Broken(e) = &rule.state {
        return Err(format!("it does not load: {e}"));
    }
    if rule.mode != jouet_rules::Mode::Warn {
        return Err("it is not in warn mode".into());
    }
    engine.proven(rule)
}

/// A command whose program and subcommands are `WORDS` and whose flags are
/// exactly `FLAGS`, whatever their values: the signature the detector saw
/// fail, so a variant with another flag (often what worked) is left alone.
const MATCHES: &str = "
local function matches(command: JouetCommand): boolean
\tif command.program ~= WORDS[1] then
\t\treturn false
\tend
\tfor i = 2, #WORDS do
\t\tif command.argv[i] ~= WORDS[i] then
\t\t\treturn false
\t\tend
\tend
\tlocal seen: { [string]: boolean } = {}
\tlocal count = 0
\tfor _, arg in jouet.options(command.argv) do
\t\tif #arg > 1 and string.sub(arg, 1, 1) == \"-\" then
\t\t\tlocal name = string.match(arg, \"^([^=]+)\") or arg
\t\t\tif not seen[name] then
\t\t\t\tseen[name] = true
\t\t\t\tcount += 1
\t\t\tend
\t\tend
\tend
\tif count ~= #FLAGS then
\t\treturn false
\tend
\tfor _, flag in FLAGS do
\t\tif not seen[flag] then
\t\t\treturn false
\t\tend
\tend
\treturn true
end
";

/// A `sleep` of at least ten seconds, or of a length the rule cannot read,
/// such as `sleep $DELAY`, or a placeholder when the rule is replayed over
/// the ledger.
const SLEEPS: &str = "
local function matches(command: JouetCommand): boolean
\tif command.program ~= \"sleep\" then
\t\treturn false
\tend
\tlocal arg = command.argv[2]
\tif not arg then
\t\treturn false
\tend
\tlocal n, unit = string.match(arg, \"^([%d.]+)([smh]?)$\")
\tlocal value = tonumber(n)
\tif not value then
\t\treturn true
\tend
\treturn value * (if unit == \"m\" then 60 elseif unit == \"h\" then 3600 else 1) >= 10
end
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wind::detect::Detector;
    use crate::wind::detect::tests::ran;
    use jouet_rules::{Budget, Engine, Mode, Options, Origin, Source, State};

    fn load(name: &str, code: &str) -> Engine {
        Engine::new(
            vec![Source {
                name: name.into(),
                origin: Origin::User(format!("{name}.luau").into()),
                code: code.into(),
                trusted: true,
            }],
            Options {
                has_command: Box::new(|_| true),
                disabled: Vec::new(),
                budget: Budget::default(),
                cache_dir: None,
            },
        )
        .unwrap()
    }

    /// Feeds commands to a detector and drafts a rule from each pattern it
    /// notices; every draft must load in warn mode and pass its examples.
    fn drafts(feed: &[(&str, &str, i32, Option<u64>)]) -> Vec<(Draft, Engine)> {
        let mut d = Detector::default();
        for (i, (session, command, code, ms)) in feed.iter().enumerate() {
            let mut e = ran(i as u64, session, command, *code);
            e.duration_ms = *ms;
            d.observe(&e, &|_| false);
        }
        d.ready()
            .map(|(_, e)| {
                let name = name_for(e);
                let draft = draft(e, &name, "2026-10-30", &[]);
                let engine = load(&name, &draft.code);
                (draft, engine)
            })
            .collect()
    }

    fn assert_sound(draft: &Draft, engine: &Engine) {
        let rule = engine.rule(&draft.name).unwrap();
        assert_eq!(rule.state, State::Enabled, "{}", draft.code);
        assert_eq!(rule.mode, Mode::Warn);
        assert_eq!(rule.scope, jouet_rules::Scope::User);
        let examples = engine.examples(rule).unwrap();
        assert!(examples.iter().any(|e| e.fires), "{}", draft.code);
        assert!(examples.iter().any(|e| !e.fires), "{}", draft.code);
        for example in &examples {
            assert!(example.ok, "{example:?}\n{}", draft.code);
        }
        assert_eq!(verify(&draft.name, &draft.code), Ok(examples.len()));
    }

    #[test]
    fn a_failing_command_becomes_a_rule_naming_what_worked() {
        let out = drafts(&[
            ("a", "mytool sync --all \"quoted\\arg\"", 127, None),
            ("a", "mytool pull", 0, None),
            ("b", "mytool sync --all=yes", 127, None),
            ("b", "cd x && mytool sync --all", 127, None),
        ]);
        assert_eq!(out.len(), 1);
        let (draft, engine) = &out[0];
        assert_eq!(draft.name, "wind-mytool-fails");
        assert!(
            draft.message.contains("`mytool <arg>` worked"),
            "{}",
            draft.message
        );
        assert!(draft.code.contains("--- mode: warn\n--- scope: user\n"));
        assert_sound(draft, engine);
        let rule = engine.rule(&draft.name).unwrap();
        let fires = |command: &str| {
            let call = jouet_rules::Call {
                tool: "Bash".into(),
                command: Some(command.into()),
                ..jouet_rules::Call::default()
            };
            engine.check(rule, &call).unwrap().0.is_some()
        };
        assert!(fires("/opt/bin/mytool sync --all=no x"));
        assert!(!fires("mytool sync x"));
        assert!(!fires("mytool sync --all --offline"));
    }

    #[test]
    fn a_variant_with_another_flag_that_worked_is_left_alone() {
        let out = drafts(&[
            ("a", "mytool sync --all", 1, None),
            ("a", "mytool sync --all --offline", 0, None),
            ("b", "mytool sync --all", 1, None),
            ("b", "mytool sync --all", 1, None),
        ]);
        assert_eq!(out.len(), 1);
        let (draft, engine) = &out[0];
        assert!(
            draft
                .message
                .contains("`mytool <arg> --all --offline` worked"),
            "{}",
            draft.message
        );
        assert_sound(draft, engine);
    }

    #[test]
    fn a_sleep_draft_passes_its_examples() {
        let long = Some(60_000);
        let out = drafts(&[
            ("a", "sleep 30", 0, long),
            ("a", "sleep 1m && ls", 0, long),
            ("b", "sleep 45", 0, long),
            ("b", "sleep 45", 0, long),
            ("b", "sleep 600", 0, long),
        ]);
        let names: Vec<&str> = out.iter().map(|(d, _)| d.name.as_str()).collect();
        assert_eq!(names, ["wind-sleep"]);
        assert!(out[0].0.why.contains("5m"), "{}", out[0].0.why);
        assert_sound(&out[0].0, &out[0].1);
    }

    #[test]
    fn strings_are_escaped_for_luau() {
        assert_eq!(lua_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(lua_str("\u{1}"), "\"\\u{1}\"");
    }
}
