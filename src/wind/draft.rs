//! Drafting a rule from a noticed pattern: a TOML rule in warn mode, whose
//! `fires` examples are the calls that motivated it and whose `quiet`
//! examples are the near misses it must leave alone: what worked instead,
//! and calls from the ledger that run the same programs. A matcher
//! expresses most patterns; a long sleep, which compares durations, gets a
//! Luau script behind its matcher.

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
    /// The rule file, in TOML.
    pub code: String,
    /// Its script, `<name>.luau`, when a matcher cannot express it.
    pub script: Option<String>,
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
    let (body, script, mut quiet) = match e.pattern {
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
            let body = format!(
                "message = {}\n\n# The command the detector saw fail, with exactly these options, whatever\n# their values: a variant with another option (often what worked) is left alone.\n[rule.match]\ncommand = {}\nexact_flags = {}\n",
                rulefile::toml_str(&message),
                rulefile::toml_str(&e.signature.words.join(" ")),
                rulefile::toml_list(&single_flags(&e.signature.flags)),
            );
            (body, None, quiet)
        }
        Pattern::Sleep => (
            format!(
                "needs = [\"wait\"]\n# A matcher cannot compare durations, so a script decides.\nscript = {}\n\n# The script only sees calls that sleep.\n[rule.match]\ncommand = \"sleep\"\nargs = \"*\"\n",
                rulefile::toml_str(&format!("{name}.luau"))
            ),
            Some(format!(
                "--!strict\n-- The `{name}` rule's script ({name}.toml), drafted by `jouet wind`.\n\nlocal MESSAGE = {}\n{SLEEPS}\nlocal rule: JouetRule = {{\n\tcheck = function(call: JouetCall): JouetDecision?\n\t\tfor _, command in call.commands do\n\t\t\tif matches(command) then\n\t\t\t\treturn jouet.deny(MESSAGE)\n\t\t\tend\n\t\tend\n\t\treturn nil\n\tend,\n}}\n\nreturn rule\n",
                lua_str(&message)
            )),
            vec!["sleep 1".to_string(), "echo sleep 60".to_string()],
        ),
    };
    for shape in near {
        let script = reader::script(shape);
        if !quiet.contains(&script) && !examples.contains(&script) {
            quiet.push(script);
        }
    }
    let code = format!(
        "{}# Drafted by `jouet wind` from this machine's hook ledger. Check it with\n# `jouet rule test {name} --replay` before relying on it.\n{header}{body}\n[rule.examples]\n# The calls that showed the pattern.\nfires = {}\n# What worked instead, and calls running the same programs.\nquiet = {}\n",
        rulefile::schema_line(),
        rulefile::toml_list(&examples),
        rulefile::toml_list(&quiet),
    );
    Draft {
        name: name.to_string(),
        why,
        message,
        code,
        script,
    }
}

/// The signature's options, one to an entry: `-fu` is `-f` and `-u`, as the
/// matcher reads them.
fn single_flags(flags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for flag in flags {
        let singles: Vec<String> = if flag.starts_with("--") {
            vec![flag.split('=').next().unwrap_or(flag).to_string()]
        } else {
            flag.chars().skip(1).map(|c| format!("-{c}")).collect()
        };
        for single in singles {
            if !out.contains(&single) {
                out.push(single);
            }
        }
    }
    out
}

/// A draft's rule, loaded on its own as a rule of yours.
fn load(draft: &Draft) -> jouet_rules::Engine {
    let name = &draft.name;
    let mut source = jouet_rules::Source::toml(
        name.clone(),
        jouet_rules::Origin::User(format!("{name}.toml").into()),
        draft.code.clone(),
    );
    if let Some(script) = &draft.script {
        source = source.with_script(format!("{name}.luau"), script.clone());
    }
    let mut engine = jouet_rules::Engine::new(
        vec![source],
        jouet_rules::Options {
            has_command: Box::new(|_| true),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    );
    engine.load_scripts();
    engine
}

/// The calls in the ledger that run the pattern's program and that a draft
/// leaves alone, most frequent first: near misses for its `quiet` examples.
pub(crate) fn near_misses(draft: &Draft, e: &Evidence, entries: &[Entry]) -> Vec<String> {
    let Some(program) = e.signature.words.first() else {
        return Vec::new();
    };
    let engine = load(draft);
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
pub(crate) fn verify(draft: &Draft) -> Result<usize, String> {
    let engine = load(draft);
    let rule = engine.rule(&draft.name).ok_or("the rule did not load")?;
    if let jouet_rules::State::Broken(e) = &rule.state {
        return Err(format!("it does not load: {e}"));
    }
    if rule.mode != jouet_rules::Mode::Warn {
        return Err("it is not in warn mode".into());
    }
    engine.proven(rule)
}

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
    use jouet_rules::{Engine, Mode, State};

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
                let engine = load(&draft);
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
        assert_eq!(verify(draft), Ok(examples.len()));
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
        assert!(draft.code.contains("mode = \"warn\"\nscope = \"user\"\n"));
        assert!(
            draft
                .code
                .contains("command = \"mytool\"\nexact_flags = [\"--all\"]\n"),
            "{}",
            draft.code
        );
        assert_eq!(draft.script, None);
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
        let script = out[0].0.script.as_deref().unwrap();
        assert!(script.contains("local MESSAGE = "), "{script}");
        assert!(out[0].0.code.contains("script = \"wind-sleep.luau\"\n"));
    }

    #[test]
    fn clusters_become_single_options() {
        let flags = |f: &[&str]| single_flags(&f.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(flags(&["-fu", "--all", "-u"]), ["-f", "-u", "--all"]);
    }

    #[test]
    fn strings_are_escaped_for_luau() {
        assert_eq!(lua_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(lua_str("\u{1}"), "\"\\u{1}\"");
    }
}
