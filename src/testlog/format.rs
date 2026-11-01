//! Failure formats declared in TOML, for a runner jouet does not know: a
//! project's own e2e script, say. Each file in a `formats/` directory, the
//! user's (`~/.config/jouet/formats`), the system's, a trusted repository's
//! `.jouet/formats` or a plugin's, is one format:
//!
//! ```toml
//! description = "The e2e runner's failed tests"
//! # A line that says a test failed; its `name` group, else its first, names it.
//! failed = ['^E2E test (?<name>\S+) failed']
//! # A line that starts a test: a failure's lines are those after the last one.
//! start = ['^>>> E2E (\S+)$']
//! # Where a failure's excerpt starts: its last line that matches, else the first.
//! excerpt = ['^E2E assertion failed: ']
//! # Lines left out of the excerpt.
//! drop = ['^\S+: \d+s$']
//!
//! [[examples]]
//! output = """
//! >>> E2E cli/test_a
//! E2E assertion failed: expected 'x'
//! E2E test cli/test_a failed
//! """
//! failures = ["cli/test_a"]
//! ```
//!
//! A format whose examples do not name the failures they list does not load.

use super::{Parsed, clean};
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
    examples: Vec<Example>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Example {
    output: String,
    failures: Vec<String>,
}

/// A declared format, its patterns compiled.
#[derive(Debug)]
pub(crate) struct Format {
    pub(crate) name: String,
    failed: Vec<Regex>,
    start: Vec<Regex>,
    excerpt: Vec<Regex>,
    drop: Vec<Regex>,
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

impl Format {
    /// The format `text` declares, named `name`, once its examples pass.
    pub(crate) fn parse(name: &str, text: &str) -> Result<Self, String> {
        let decl: Decl = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if decl.failed.is_empty() {
            return Err("`failed` needs at least one pattern".into());
        }
        let failed = compile("failed", &decl.failed)?;
        if let Some(p) = failed.iter().find(|p| p.captures_len() < 2) {
            return Err(format!(
                "`failed` pattern `{p}` needs a group that names the test, such as `(?<name>\\S+)`"
            ));
        }
        let format = Self {
            name: name.to_string(),
            failed,
            start: compile("start", &decl.start)?,
            excerpt: compile("excerpt", &decl.excerpt)?,
            drop: compile("drop", &decl.drop)?,
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
        }
        Ok(format)
    }

    /// Adds each failed test `lines` have by this format, with its excerpt.
    pub(super) fn apply(&self, lines: &[String], out: &mut Parsed) {
        let mut start = 0;
        let mut failed = 0;
        for (i, line) in lines.iter().enumerate() {
            if any(&self.start, line) {
                start = i + 1;
                continue;
            }
            let Some(c) = self.failed.iter().find_map(|p| p.captures(line)) else {
                continue;
            };
            let Some(name) = c.name("name").or_else(|| c.get(1)) else {
                continue;
            };
            failed += 1;
            let body = &lines[start.min(i)..i];
            let from = body
                .iter()
                .rposition(|l| any(&self.excerpt, l))
                .unwrap_or(0);
            let block = body[from..]
                .iter()
                .filter(|l| !any(&self.drop, l))
                .cloned()
                .collect();
            out.push(i, name.as_str(), block);
            start = i + 1;
        }
        out.count(0, failed, 0);
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
        let Ok(entries) = std::fs::read_dir(set.formats_dir()) else {
            continue;
        };
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
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
