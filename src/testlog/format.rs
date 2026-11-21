//! Failure formats declared in TOML, for a runner jouet does not know: a
//! project's own e2e script, say. Each file in a `formats/` directory, the
//! user's (`~/.config/jouet/formats`), the system's, a trusted repository's
//! `.jouet/formats` or a plugin's, is one format:
//!
//! ```toml
//! description = "The e2e runner's failed tests"
//! # A line that says a test failed; its `name` group, else its first, names it.
//! failed = ['^E2E test (?<name>\S+) failed']
//! # A line that starts a test: a failure's lines are those after the last
//! # one. Without it, a failure has only the lines from an excerpt line on, or
//! # else the failed line itself.
//! start = ['^>>> E2E (\S+)$']
//! # Where a failure's excerpt starts: its last line that matches, else the first.
//! excerpt = ['^E2E assertion failed: ']
//! # Lines left out of the excerpt.
//! drop = ['^\S+: \d+s$']
//! # The runner's own count of the tests it ran: its first group, else the
//! # whole line, is read for `3 passed`, `1 failed` and `2 skipped`.
//! summary = ['^E2E: ran \d+ tests \((.+)\)']
//! # A line that says a test passed, named as `failed` names one. With it, a
//! # run that was cut short (a time limit, a signal) names the tests that had
//! # started and neither passed nor failed, which takes a `start` pattern
//! # with a group. Without it, only the last test started can be unfinished.
//! passed = ['^PASS: (?<name>\S+)']
//! # The command that runs one test alone, `{name}` being the test: the next
//! # command a run that was cut short gives for an unfinished test. jouet
//! # quotes the name for the shell, so `{name}` stands outside any quotes.
//! rerun = "mise run test:e2e {name}"
//!
//! [[examples]]
//! output = """
//! >>> E2E cli/test_a
//! E2E assertion failed: expected 'x'
//! E2E test cli/test_a failed
//! >>> E2E cli/test_b
//! """
//! failures = ["cli/test_a"]
//! unfinished = ["cli/test_b"]
//! ```
//!
//! A format whose examples do not name the failures they list, or the
//! unfinished tests of one that lists them, does not load.

use super::{Parsed, Unfinished, clean};
use crate::sources;
use regex::Regex;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decl {
    /// For the person reading the file; jouet does not show it.
    #[serde(default, rename = "description")]
    _description: Option<String>,
    failed: Vec<String>,
    #[serde(default)]
    start: Vec<String>,
    #[serde(default)]
    excerpt: Vec<String>,
    #[serde(default)]
    drop: Vec<String>,
    #[serde(default)]
    summary: Vec<String>,
    #[serde(default)]
    passed: Vec<String>,
    #[serde(default)]
    rerun: Option<String>,
    #[serde(default)]
    examples: Vec<Example>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Example {
    output: String,
    #[serde(default)]
    failures: Vec<String>,
    /// The tests the output leaves unfinished; not checked when left out,
    /// since an example of a whole run need not end its last test.
    #[serde(default)]
    unfinished: Option<Vec<String>>,
}

/// Where `rerun` takes the test's name.
const NAME: &str = "{name}";

/// A declared format, its patterns compiled.
#[derive(Debug)]
pub(crate) struct Format {
    pub(crate) name: String,
    failed: Vec<Regex>,
    start: Vec<Regex>,
    excerpt: Vec<Regex>,
    drop: Vec<Regex>,
    summary: Vec<Regex>,
    passed: Vec<Regex>,
    rerun: Option<String>,
}

/// The test a line names: its `name` group, else its first.
fn named<'a>(patterns: &[Regex], line: &'a str) -> Option<&'a str> {
    let c = patterns.iter().find_map(|p| p.captures(line))?;
    c.name("name").or_else(|| c.get(1)).map(|m| m.as_str())
}

/// A pattern of `key` that has no group to name the test with.
fn unnamed(key: &str, patterns: &[Regex]) -> Result<(), String> {
    match patterns.iter().find(|p| p.captures_len() < 2) {
        Some(p) => Err(format!(
            "`{key}` pattern `{p}` needs a group that names the test, such as `(?<name>\\S+)`"
        )),
        None => Ok(()),
    }
}

fn compile(key: &str, patterns: &[String]) -> Result<Vec<Regex>, String> {
    patterns
        .iter()
        .map(|p| Regex::new(p).map_err(|e| format!("`{key}` pattern `{p}`: {e}")))
        .collect()
}

fn any(patterns: &[Regex], line: &str) -> bool {
    patterns.iter().any(|p| p.is_match(line))
}

/// Whether a `{name}` of `rerun` stands inside quotes, as a shell reads
/// the command: the name is quoted when it is filled in, and quotes of its
/// own would end those around it.
fn name_in_quotes(rerun: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in rerun.char_indices() {
        if quote.is_some() && rerun[i..].starts_with(NAME) {
            return true;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            // Nothing escapes inside single quotes.
            (None | Some('"'), '\\') => escaped = true,
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), _) if open == c => quote = None,
            _ => {}
        }
    }
    false
}

impl Format {
    /// The format `text` declares, named `name`, once its examples pass.
    pub(crate) fn parse(name: &str, text: &str) -> Result<Self, String> {
        let decl: Decl = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if decl.failed.is_empty() {
            return Err("`failed` needs at least one pattern".into());
        }
        let failed = compile("failed", &decl.failed)?;
        unnamed("failed", &failed)?;
        let passed = compile("passed", &decl.passed)?;
        unnamed("passed", &passed)?;
        let start = compile("start", &decl.start)?;
        if let Some(rerun) = &decl.rerun {
            if !rerun.contains(NAME) {
                return Err(format!(
                    "`rerun` needs `{NAME}`, where the test's name goes"
                ));
            }
            if name_in_quotes(rerun) {
                return Err(format!(
                    "`rerun` has `{NAME}` inside quotes; jouet quotes the name itself, so write it outside them"
                ));
            }
            // Only a test a `start` line names can be left unfinished.
            if !start.iter().any(|p| p.captures_len() > 1) {
                return Err(
                    "`rerun` needs a `start` pattern with a group that names the test, such as `(?<name>\\S+)`"
                        .into(),
                );
            }
        }
        let format = Self {
            name: name.to_string(),
            failed,
            start,
            excerpt: compile("excerpt", &decl.excerpt)?,
            drop: compile("drop", &decl.drop)?,
            summary: compile("summary", &decl.summary)?,
            passed,
            rerun: decl.rerun,
        };
        for (i, example) in decl.examples.iter().enumerate() {
            let mut parsed = Parsed::default();
            format.apply(&clean(&example.output), &mut parsed);
            let named: Vec<String> = parsed.found.into_iter().map(|f| f.name).collect();
            if named != example.failures {
                return Err(format!(
                    "example {} names {named:?}, not {:?}",
                    i + 1,
                    example.failures
                ));
            }
            let open: Vec<String> = parsed.unfinished.into_iter().map(|u| u.name).collect();
            if let Some(unfinished) = &example.unfinished
                && &open != unfinished
            {
                return Err(format!(
                    "example {} leaves {open:?} unfinished, not {unfinished:?}",
                    i + 1
                ));
            }
        }
        Ok(format)
    }

    /// Adds each failed test `lines` have by this format, with its excerpt,
    /// the counts its summary lines give, or else one failure and one pass
    /// for each that no summary line after it counts, and the tests that
    /// started and did not end.
    pub(super) fn apply(&self, lines: &[String], out: &mut Parsed) {
        let mut start = 0;
        // The failures and passes since the last summary line, which it did
        // not count.
        let mut uncounted = 0;
        let mut uncounted_passes = 0;
        // Every line that starts a test, and the tests still running, by the
        // line each started on.
        let mut starts: Vec<usize> = Vec::new();
        let mut open: Vec<(usize, &str)> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            // A summary line can also start a test or name a failure, so it
            // is read for those too. One that a runner jouet knows has
            // counted is not counted again.
            let summary = self.summary.iter().find_map(|p| p.captures(line));
            if let Some(c) = &summary {
                if !super::known_runners_count(line) {
                    out.count_words(c.get(1).map_or(line.as_str(), |g| g.as_str()));
                }
                uncounted = 0;
                uncounted_passes = 0;
                // A run that summed up left no test running.
                open.clear();
            }
            if any(&self.start, line) {
                start = i + 1;
                starts.push(i);
                // A runner that does not say when a test passes runs one at
                // a time, as far as its output tells: the next start ends
                // the last.
                if self.passed.is_empty() {
                    open.clear();
                }
                if let Some(name) = named(&self.start, line) {
                    open.retain(|(_, n)| *n != name);
                    open.push((i, name));
                }
                continue;
            }
            if let Some(name) = named(&self.passed, line) {
                out.passed_names.push(name.to_string());
                if summary.is_none() {
                    uncounted_passes += 1;
                }
                open.retain(|(_, n)| *n != name);
                continue;
            }
            let Some(name) = named(&self.failed, line) else {
                continue;
            };
            if self.passed.is_empty() {
                open.clear();
            } else {
                open.retain(|(_, n)| *n != name);
            }
            if summary.is_none() {
                uncounted += 1;
            }
            let body = &lines[start.min(i)..i];
            // Without a start, the lines before a failure may be another
            // test's: only an excerpt line marks where this one's begin.
            let from = body
                .iter()
                .rposition(|l| any(&self.excerpt, l))
                .unwrap_or(if self.start.is_empty() { body.len() } else { 0 });
            let mut block: Vec<String> = body[from..]
                .iter()
                // A test run beside this one may have passed meanwhile.
                .filter(|l| !any(&self.drop, l) && !any(&self.passed, l))
                .cloned()
                .collect();
            // A runner that says it all on the failed line (`FAIL a: expected
            // 1, got 2`) has nothing before it to show.
            if block.is_empty() {
                block.push(line.clone());
            }
            out.push(i, name, block);
            start = i + 1;
        }
        out.count(uncounted_passes, uncounted, 0);
        for (at, name) in open {
            // Its lines end where the next test starts: tests run side by
            // side share the output, less the lines that say another passed.
            let end = starts.iter().find(|&&s| s > at).map_or(lines.len(), |&s| s);
            out.unfinished.push(Unfinished {
                name: name.to_string(),
                lines: lines[at + 1..end]
                    .iter()
                    .filter(|l| !any(&self.drop, l) && !any(&self.passed, l))
                    .cloned()
                    .collect(),
                rerun: self
                    .rerun
                    .as_ref()
                    .map(|rerun| rerun.replace(NAME, &crate::affected::quote(name))),
            });
        }
    }
}

/// The formats that load for `cwd`, from each set that runs, a plugin's
/// named after it (`acme/e2e`), and what is wrong with any that do not.
pub(crate) fn load(cwd: &Path) -> (Vec<Format>, Vec<String>) {
    // Pins that cannot be followed are `jouet rule list`'s to report.
    let sets = sources::sets(cwd, None, &mut sources::digests(), &mut Vec::new());
    let mut errors = Vec::new();
    let mut formats = Vec::new();
    for set in sets.iter().filter(|s| s.loads() && s.status.runs()) {
        let dir = set.formats_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push(format!("formats {}: {e}", dir.display()));
                continue;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(e) => errors.push(format!("formats {}: {e}", dir.display())),
            }
        }
        paths.retain(|p| p.extension().is_some_and(|x| x == "toml"));
        paths.sort();
        for path in paths {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let name = match set.namespace() {
                Some(ns) => format!("{ns}/{stem}"),
                None => stem.into_owned(),
            };
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| Format::parse(&name, &text));
            match parsed {
                Ok(format) => formats.push(format),
                Err(e) => errors.push(format!("format {}: {e}", path.display())),
            }
        }
    }
    (formats, errors)
}
