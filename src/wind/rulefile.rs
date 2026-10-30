//! Changing a user's rule files: writing a new one, switching its mode,
//! retiring it, and the log every such change is written to. A rule is a
//! `[[rule]]` table in a TOML file; a deprecated Luau rule file keeps its
//! `--- key: value` header.
//!
//! Only rules in `~/.config/jouet/rules` are ever written. Built-in rules are
//! part of the binary, and a repository's rules change through a reviewed PR.

use jouet_rules::Mode;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::io::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Whether a name is fit for a rule: lowercase words joined by dashes.
pub(crate) fn valid_name(name: &str) -> bool {
    jouet_rules::valid_name(name)
}

pub(crate) struct Header<'a> {
    pub name: &'a str,
    pub why: &'a str,
    pub author: &'a str,
    pub created: &'a str,
}

/// A TOML basic string.
pub(crate) fn toml_str(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A TOML array of strings, one to a line when they do not fit on one.
pub(crate) fn toml_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| toml_str(i)).collect();
    let one_line = format!("[{}]", quoted.join(", "));
    if one_line.len() <= 72 && !one_line.contains("\\n") {
        return one_line;
    }
    let mut out = String::from("[\n");
    for item in quoted {
        let _ = writeln!(out, "  {item},");
    }
    out.push(']');
    out
}

/// The line every rule file jouet writes starts with, so an editor finds
/// the schema.
pub(crate) fn schema_line() -> String {
    format!("#:schema {}\n", jouet_rules::SCHEMA_URL)
}

/// A rule's `[[rule]]` line and the keys that say who it is: always in
/// warn mode and user scope, since that is the only kind of rule jouet
/// writes.
pub(crate) fn header(h: &Header) -> String {
    format!(
        "[[rule]]\nname = {}\nwhy = {}\nmode = \"warn\"\nscope = \"user\"\nauthor = {}\ncreated = {}\n",
        toml_str(h.name),
        toml_str(&one_line(h.why)),
        toml_str(&one_line(h.author)),
        toml_str(h.created)
    )
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The examples table every new rule starts with, empty.
const EXAMPLES: &str = "
# Fill these in first: the rule only warns, and cannot be promoted, until
# it passes at least one of each.
[rule.examples]
# Calls the rule must fire on.
fires = []
# Near misses it must leave alone.
quiet = []
";

/// A new rule to fill in: a matcher, or with `script`, a matcher in front
/// of a Luau script, whose file is the second value.
pub(crate) fn scaffold(h: &Header, script: bool) -> (String, Option<String>) {
    let name = h.name;
    let intro = format!(
        "{}# `jouet rule schema` describes every key, and `jouet rule test {name} --replay`\n# runs this rule over the calls jouet's hooks have seen.\n{}",
        schema_line(),
        header(h)
    );
    if !script {
        let toml = format!(
            "{intro}# What the agent is told, and the command to run instead, if there is one.\nmessage = \"Say why, and what to do instead.\"\n# suggest = \"the command to run instead\"\n\n# The calls it fires on: the program and subcommands, then anything else\n# that must hold, such as `flags = [\"-f\", \"--force\"]` or `args = [\".\"]`.\n[rule.match]\ncommand = \"replace-me\"\n{EXAMPLES}"
        );
        return (toml, None);
    }
    let toml = format!(
        "{intro}script = {}\n\n# The script runs only on calls this matches; leave it out to run it on all.\n[rule.match]\ncommand = \"replace-me\"\n{EXAMPLES}",
        toml_str(&format!("{name}.luau"))
    );
    let luau = format!(
        r#"--!strict
-- The `{name}` rule's script ({name}.toml), for logic its matcher cannot
-- express. `jouet rule types` prints the API.
local rule: JouetRule = {{
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			-- Decide about the command this rule is about, for example:
			-- if command.program == "git" and command.argv[2] == "stash" then
			if false then
				return jouet.deny("Say why, and what to do instead.")
			end
		end
		return nil
	end,
}}

return rule
"#
    );
    (toml, Some(luau))
}

/// The byte range of the `[[rule]]` table named `rule` in a TOML file,
/// with the comment lines right above it, or None when the file does not
/// write it as a `[[rule]]` table.
pub(crate) fn block(code: &str, rule: &str) -> Option<Range<usize>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        if line.trim() == "[[rule]]" {
            starts.push(offset);
        }
        offset += line.len();
    }
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(code.len());
        let keys = &code[start..end];
        if top_keys(keys).any(|(key, value, _)| key == "name" && value.as_deref() == Some(rule)) {
            return Some(with_comments(code, start)..end);
        }
    }
    None
}

/// Moves a block's start up over the comment lines right above it, but not
/// over a `#:` directive such as `#:schema`.
fn with_comments(code: &str, mut start: usize) -> usize {
    while start > 0 {
        let before = &code[..start - 1];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let line = code[line_start..start].trim();
        if !line.starts_with('#') || line.starts_with("#:") {
            break;
        }
        start = line_start;
    }
    start
}

/// The keys of a `[[rule]]` table's own lines, before its first subtable:
/// each key, its value when it is a string, and its line's byte range in
/// `block`.
fn top_keys(block: &str) -> impl Iterator<Item = (String, Option<String>, Range<usize>)> + '_ {
    let mut offset = 0;
    block
        .split_inclusive('\n')
        .map(move |line| {
            let range = offset..offset + line.len();
            offset += line.len();
            (line, range)
        })
        .skip_while(|(line, _)| line.trim() != "[[rule]]")
        .skip(1)
        .take_while(|(line, _)| !line.trim_start().starts_with('['))
        .filter_map(|(line, range)| {
            let table: toml::Table = toml::from_str(line).ok()?;
            let (key, value) = table.into_iter().next()?;
            Some((key, value.as_str().map(str::to_string), range))
        })
}

/// The file with one string key of the rule named `rule` set, adding it
/// after `why` when the rule has none. Fails when the file does not write
/// the rule as a `[[rule]]` table jouet can edit.
pub(crate) fn set_field(code: &str, rule: &str, key: &str, value: &str) -> Result<String, String> {
    let range = block(code, rule).ok_or_else(|| {
        format!("`{rule}` is not a `[[rule]]` table jouet can edit; set `{key}` by hand")
    })?;
    let rule_block = &code[range.clone()];
    let line = format!("{key} = {}\n", toml_str(&one_line(value)));
    let keys: Vec<_> = top_keys(rule_block).collect();
    let at = keys
        .iter()
        .find(|(k, _, _)| k == key)
        .map(|(_, _, r)| r.clone())
        .or_else(|| {
            keys.iter()
                .find(|(k, _, _)| k == "why")
                .map(|(_, _, r)| r.end..r.end)
        })
        .ok_or_else(|| format!("`{rule}` has no `why`; set `{key}` by hand"))?;
    let at = range.start + at.start..range.start + at.end;
    let mut out = String::with_capacity(code.len() + line.len());
    out.push_str(&code[..at.start]);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line);
    out.push_str(&code[at.end..]);
    Ok(out)
}

/// The file with a rule's mode set.
pub(crate) fn set_mode(code: &str, rule: &str, mode: Mode) -> Result<String, String> {
    set_field(code, rule, "mode", mode_word(mode))
}

fn mode_word(mode: Mode) -> &'static str {
    match mode {
        Mode::Warn => "warn",
        Mode::Deny => "deny",
    }
}

/// A Luau rule file's code with its header's mode set.
pub(crate) fn set_luau_mode(code: &str, mode: Mode) -> String {
    set_luau_field(code, "mode", mode_word(mode))
}

/// A Luau rule file's code with one header field set, adding it after `why`
/// when the header has none. A file without a header is returned as it is.
pub(crate) fn set_luau_field(code: &str, key: &str, value: &str) -> String {
    let line = format!("--- {key}: {}\n", one_line(value));
    // The header's lines, with each one's field name, up to the first line
    // that is not part of it.
    let lines: Vec<&str> = code.split_inclusive('\n').collect();
    let field = |l: &str| {
        let t = l.trim();
        t.strip_prefix("---")
            .and_then(|f| f.split_once(':'))
            .map(|(k, _)| k.trim().to_string())
    };
    let header_len = lines
        .iter()
        .position(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("--!") && field(l).is_none()
        })
        .unwrap_or(lines.len());
    let header = &lines[..header_len];
    let at = header
        .iter()
        .position(|l| field(l).as_deref() == Some(key))
        .map(|i| (i, true))
        .or_else(|| {
            header
                .iter()
                .position(|l| field(l).as_deref() == Some("why"))
                .map(|i| (i + 1, false))
        });
    let Some((at, replace)) = at else {
        return code.to_string();
    };
    let mut out: String = lines[..at].concat();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line);
    let rest = if replace { at + 1 } else { at };
    out.push_str(&lines[rest..].concat());
    out
}

/// Where retired rules go: beside the rules, where they no longer load.
pub(crate) fn retired_dir(rules_dir: &Path) -> PathBuf {
    rules_dir.join("retired")
}

/// Moves a file out of the directory rules load from. Returns where it
/// went.
pub(crate) fn retire(path: &Path) -> std::io::Result<PathBuf> {
    let target = free_target(path)?;
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// Where a file retires to: `retired/` beside it, under a name no retired
/// file has yet.
fn free_target(path: &Path) -> std::io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let retired = retired_dir(dir);
    std::fs::create_dir_all(&retired)?;
    let name = path.file_name().unwrap_or_default();
    let mut target = retired.join(name);
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = retired.join(format!("{}.{n}", name.to_string_lossy()));
    }
    Ok(target)
}

/// Retires one rule from the file at `path`: the whole file (and the
/// script it names, if any) when the rule is all it holds, or else just
/// the rule's table, which moves to `retired/<rule>.toml`. Returns where
/// it went.
pub(crate) fn retire_rule(
    path: &Path,
    rule: &str,
    script: Option<&str>,
) -> std::io::Result<PathBuf> {
    if path.extension().is_some_and(|e| e == "luau") {
        return retire(path);
    }
    let code = std::fs::read_to_string(path)?;
    let tables = code.lines().filter(|l| l.trim() == "[[rule]]").count();
    let dir = path.parent().unwrap_or(Path::new("."));
    if tables <= 1 {
        let target = retire(path)?;
        if let Some(script) = script
            && dir.join(script).is_file()
        {
            retire(&dir.join(script))?;
        }
        return Ok(target);
    }
    let range = block(&code, rule).ok_or_else(|| {
        std::io::Error::other(format!(
            "`{rule}` shares {} with other rules and is not a `[[rule]]` table jouet can move; remove it by hand",
            path.display()
        ))
    })?;
    let target = free_target(&dir.join(format!("{rule}.toml")))?;
    std::fs::write(
        &target,
        format!("{}{}", schema_line(), &code[range.clone()]),
    )?;
    let mut rest = code[..range.start].to_string();
    rest.push_str(&code[range.end..]);
    std::fs::write(path, rest)?;
    Ok(target)
}

/// One change to a rule, as `jouet rule log` prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Change {
    pub ts: u64,
    pub rule: String,
    /// `created`, `promoted`, `retired` or `shared`.
    pub change: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub reason: String,
    /// Who made the change: a session, `user`, or `jouet wind`.
    pub by: String,
}

/// `$XDG_STATE_HOME/jouet/rule-log.jsonl`.
pub(crate) fn log_path() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("rule-log.jsonl"))
}

pub(crate) fn log(path: &Path, change: &Change) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_vec(change)?;
    line.push(b'\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)
}

/// Every change in the log, oldest first; unreadable lines are skipped.
pub(crate) fn read_log(path: &Path) -> Vec<Change> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Header = Header {
        name: "my-rule",
        why: "because\nreasons",
        author: "session-1",
        created: "2026-10-30",
    };

    fn engine(name: &str, code: &str, script: Option<&str>) -> jouet_rules::Engine {
        let mut source = jouet_rules::Source::toml(
            name,
            jouet_rules::Origin::User(format!("{name}.toml").into()),
            code,
        );
        if let Some(script) = script {
            source = source.with_script(format!("{name}.luau"), script);
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

    #[test]
    fn names() {
        for good in ["a", "wind-cargo-127", "x9"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-a", "a-", "A", "a_b", "a/b", "../x", "9a"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn strings_are_escaped_for_toml() {
        assert_eq!(toml_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(toml_str("\u{1}"), "\"\\u0001\"");
        let parsed: toml::Table =
            toml::from_str(&format!("k = {}", toml_str("x\"\\\n\u{7f}y"))).unwrap();
        assert_eq!(parsed["k"].as_str(), Some("x\"\\\n\u{7f}y"));
        let long: Vec<String> = (0..20).map(|i| format!("item {i}")).collect();
        let parsed: toml::Table = toml::from_str(&format!("k = {}", toml_list(&long))).unwrap();
        assert_eq!(parsed["k"].as_array().unwrap().len(), 20);
    }

    #[test]
    fn the_scaffolds_load_in_warn_mode_with_their_header() {
        let (code, script) = scaffold(&H, false);
        assert!(script.is_none());
        assert!(code.starts_with("#:schema https://jouet.jdx.dev/schema/rules.json\n"));
        assert!(code.contains("why = \"because reasons\"\n"), "{code}");
        let plain = engine("my-rule", &code, None);
        let rule = plain.rule("my-rule").unwrap();
        assert_eq!(rule.state, jouet_rules::State::Enabled, "{code}");
        assert_eq!(rule.mode, Mode::Warn);
        assert_eq!(rule.author, "session-1");
        assert!(rule.missing_examples.is_some());

        let (code, script) = scaffold(&H, true);
        let scripted = engine("my-rule", &code, script.as_deref());
        let rule = scripted.rule("my-rule").unwrap();
        assert_eq!(rule.state, jouet_rules::State::Enabled, "{code}");
        assert_eq!(rule.script.as_deref(), Some("my-rule.luau"));
    }

    const TWO: &str = "#:schema x\n# The first.\n[[rule]]\nname = \"a\"\nwhy = \"w\"\nmessage = \"m\"\n\n[rule.match]\ncommand = \"ls\"\nmode = \"x\"\n\n[[rule]]\nname = \"b\"\nwhy = \"w\"\nmode = \"warn\"\nmessage = \"m\"\n[rule.match]\ncommand = \"rm\"\n";

    #[test]
    fn set_mode_rewrites_or_adds_the_rules_own_line() {
        let a = set_mode(TWO, "a", Mode::Deny).unwrap();
        assert!(a.contains("why = \"w\"\nmode = \"deny\"\nmessage"), "{a}");
        // The subtable's `mode` key is not the rule's.
        assert!(a.contains("command = \"ls\"\nmode = \"x\""), "{a}");
        let b = set_mode(TWO, "b", Mode::Deny).unwrap();
        assert!(
            b.contains("name = \"b\"\nwhy = \"w\"\nmode = \"deny\"\n"),
            "{b}"
        );
        assert!(!b.contains("mode = \"warn\""));
        assert_eq!(set_mode(&b, "b", Mode::Warn).unwrap(), TWO);
        assert!(set_mode(TWO, "c", Mode::Deny).is_err());
        assert!(set_mode("rule = [{ name = \"a\", why = \"w\" }]", "a", Mode::Deny).is_err());
        let author = set_field(TWO, "a", "author", "session-9").unwrap();
        assert!(
            author.contains("why = \"w\"\nauthor = \"session-9\"\n"),
            "{author}"
        );
    }

    #[test]
    fn a_block_takes_the_comments_above_it_but_not_the_schema_line() {
        let range = block(TWO, "a").unwrap();
        assert!(TWO[range.clone()].starts_with("# The first.\n[[rule]]"));
        assert!(TWO[range].ends_with("mode = \"x\"\n\n"));
        assert!(TWO[block(TWO, "b").unwrap()].starts_with("[[rule]]\nname = \"b\""));
    }

    #[test]
    fn set_luau_mode_rewrites_or_adds_the_header_line() {
        let code = "--!strict\n--- why: x\n--- mode: warn\nreturn {}\n";
        let deny = set_luau_mode(code, Mode::Deny);
        assert!(deny.contains("--- mode: deny\n") && !deny.contains("mode: warn"));
        assert_eq!(set_luau_mode(&deny, Mode::Warn), code);
        let bare = "--- why: x\nreturn {}\n-- --- mode: warn\n";
        assert_eq!(
            set_luau_mode(bare, Mode::Deny),
            "--- why: x\n--- mode: deny\nreturn {}\n-- --- mode: warn\n"
        );
    }

    #[test]
    fn retiring_moves_the_file_or_the_rule_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.luau");
        std::fs::write(&path, "x").unwrap();
        let first = retire(&path).unwrap();
        std::fs::write(&path, "y").unwrap();
        let second = retire(&path).unwrap();
        assert_ne!(first, second);
        assert!(!path.exists());
        assert!(jouet_rules::rule_files(dir.path()).unwrap().is_empty());

        let one = dir.path().join("one.toml");
        std::fs::write(
            &one,
            "[[rule]]\nname = \"one\"\nwhy = \"w\"\nscript = \"one.luau\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("one.luau"), "return {}").unwrap();
        let to = retire_rule(&one, "one", Some("one.luau")).unwrap();
        assert_eq!(to, dir.path().join("retired").join("one.toml"));
        assert!(dir.path().join("retired").join("one.luau").is_file());

        let two = dir.path().join("two.toml");
        std::fs::write(&two, TWO).unwrap();
        let to = retire_rule(&two, "a", None).unwrap();
        assert_eq!(to, dir.path().join("retired").join("a.toml"));
        let left = std::fs::read_to_string(&two).unwrap();
        assert!(
            left.starts_with("#:schema x\n[[rule]]\nname = \"b\""),
            "{left}"
        );
        assert!(
            std::fs::read_to_string(&to)
                .unwrap()
                .contains("name = \"a\"")
        );
        assert!(jouet_rules::rule_files(dir.path()).unwrap().len() == 1);
    }
}
