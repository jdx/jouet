//! Which findings an edit introduced.
//!
//! Two sets of findings are compared file by file. A finding after the edit
//! is the same as one before it when the file, rule and step match and
//! either the message matches (digits aside, so a moved line number in a
//! message does not count) or the line is within [`NEAR`] lines. The first
//! catches a finding that moved because lines were added above it; the
//! second, one whose message changed with the code around it. What is left
//! is new.
//!
//! The before side is an explicit run (`jouet edit --diag` checks the files
//! before writing them) or the cache: every check records, per file, the
//! file's hash and what was found in it, keeping the last two versions, so
//! `jouet diag delta` compares with the newest result for a different
//! version of the file. A file unchanged since HEAD has nothing new by
//! definition.

use super::{Finding, Report, Step};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How many lines a finding may move and still be the same one.
const NEAR: u64 = 3;
/// How many files the cache remembers per repository.
const MAX_FILES: usize = 1000;

/// What changed between two sets of findings.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Comparison {
    pub new: Vec<Finding>,
    pub kept: usize,
    pub fixed: usize,
}

pub(crate) fn compare(before: &[Finding], after: &[Finding]) -> Comparison {
    let mut used = vec![false; before.len()];
    let mut matched = vec![false; after.len()];
    for pass in [same_message as Pass, nearby] {
        // Closest pairs first, so a new finding next to an old one cannot
        // take the match of the one that only moved.
        let mut pairs: Vec<(u64, usize, usize)> = Vec::new();
        for (i, a) in after.iter().enumerate() {
            for (j, b) in before.iter().enumerate() {
                if !matched[i] && !used[j] && pass(a, b) {
                    pairs.push((distance(a, b), i, j));
                }
            }
        }
        pairs.sort_unstable();
        for (_, i, j) in pairs {
            if !matched[i] && !used[j] {
                matched[i] = true;
                used[j] = true;
            }
        }
    }
    Comparison {
        new: after
            .iter()
            .zip(&matched)
            .filter(|(_, m)| !**m)
            .map(|(a, _)| a.clone())
            .collect(),
        kept: matched.iter().filter(|m| **m).count(),
        fixed: used.iter().filter(|u| !**u).count(),
    }
}

/// One way two findings can be the same one.
type Pass = fn(&Finding, &Finding) -> bool;

fn same_message(a: &Finding, b: &Finding) -> bool {
    same_kind(a, b) && words(&a.message) == words(&b.message)
}

fn nearby(a: &Finding, b: &Finding) -> bool {
    same_kind(a, b) && distance(a, b) <= NEAR
}

fn same_kind(a: &Finding, b: &Finding) -> bool {
    a.file == b.file && a.rule == b.rule && a.source_step == b.source_step
}

fn distance(a: &Finding, b: &Finding) -> u64 {
    match (a.line, b.line) {
        (Some(x), Some(y)) => x.abs_diff(y),
        (None, None) => 0,
        _ => u64::MAX,
    }
}

/// The message with each run of digits replaced, so "line 12" and "line 14"
/// compare equal.
fn words(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut digits = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            out.push(c);
            digits = false;
        }
    }
    out
}

/// What one check found in one version of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Seen {
    pub hash: String,
    pub findings: Vec<Finding>,
    /// Steps that failed with no findings in the run that checked it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_steps: Vec<String>,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    last: Seen,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prev: Option<Seen>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Cache {
    #[serde(default)]
    files: BTreeMap<String, Entry>,
}

/// The cache file for the repository at `root`.
pub(crate) fn cache_path(root: &Path) -> Option<PathBuf> {
    let key = crate::ledger::repo_key(root)?;
    Some(
        jouet_output::paths::state_dir()?
            .join("diag")
            .join(format!("{key}.json")),
    )
}

/// A file's SHA-256, or None when it cannot be read.
pub(crate) fn hash(root: &Path, file: &str) -> Option<String> {
    std::fs::read(root.join(file))
        .ok()
        .map(|b| jouet_edit::sha256_hex(&b))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a report found in each of its files, as the cache keeps it.
pub(crate) fn seen(root: &Path, report: &Report) -> Vec<(String, Seen)> {
    let failed: Vec<String> = report
        .failed_steps()
        .filter(|s| s.findings == 0)
        .map(|s| s.name.clone())
        .collect();
    report
        .files
        .iter()
        .filter_map(|file| {
            let seen = Seen {
                hash: hash(root, file)?,
                findings: report
                    .findings
                    .iter()
                    .filter(|f| &f.file == file)
                    .cloned()
                    .collect(),
                failed_steps: failed.clone(),
                at: now(),
            };
            Some((file.clone(), seen))
        })
        .collect()
}

impl Cache {
    pub(crate) fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The newest result for a version of `file` other than `hash`.
    pub(crate) fn before(&self, file: &str, hash: &str) -> Option<&Seen> {
        let entry = self.files.get(file)?;
        if entry.last.hash != hash {
            return Some(&entry.last);
        }
        entry.prev.as_ref().filter(|p| p.hash != hash)
    }

    /// The result for exactly this version of `file`.
    pub(crate) fn at(&self, file: &str, hash: &str) -> Option<&Seen> {
        let entry = self.files.get(file)?;
        [Some(&entry.last), entry.prev.as_ref()]
            .into_iter()
            .flatten()
            .find(|s| s.hash == hash)
    }

    pub(crate) fn record(&mut self, results: Vec<(String, Seen)>) {
        for (file, seen) in results {
            match self.files.get_mut(&file) {
                Some(entry) if entry.last.hash == seen.hash => entry.last = seen,
                Some(entry) => {
                    let last = std::mem::replace(&mut entry.last, seen);
                    entry.prev = Some(last);
                }
                None => {
                    self.files.insert(
                        file,
                        Entry {
                            last: seen,
                            prev: None,
                        },
                    );
                }
            }
        }
        while self.files.len() > MAX_FILES {
            let oldest = self
                .files
                .iter()
                .min_by_key(|(_, e)| e.last.at)
                .map(|(f, _)| f.clone());
            match oldest {
                Some(file) => self.files.remove(&file),
                None => break,
            };
        }
    }
}

/// Loads, changes and saves the cache under a lock, so two checks finishing
/// at once keep both results. Best effort.
pub(crate) fn update(path: &Path, change: impl FnOnce(&mut Cache)) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
    else {
        return;
    };
    if lock.lock().is_err() {
        return;
    }
    let mut cache = Cache::load(path);
    change(&mut cache);
    let Ok(mut file) = tempfile::NamedTempFile::new_in(dir) else {
        return;
    };
    if serde_json::to_writer(&mut file, &cache).is_ok() {
        let _ = file.persist(path);
    }
}

/// The findings and failed steps an edit introduced, against a baseline.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Delta {
    /// Findings the baseline did not have.
    pub new: Vec<Finding>,
    /// Steps with no findings that failed now and not in the baseline.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new_failed_steps: Vec<Step>,
    /// Findings the baseline had too.
    pub kept: usize,
    /// Baseline findings that are gone.
    pub fixed: usize,
    /// Files changed since HEAD that no earlier check saw in another
    /// version: their findings are not counted as new.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub no_baseline: Vec<String>,
    /// Findings in those files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<Finding>,
}

impl Delta {
    pub(crate) fn introduced(&self) -> bool {
        !self.new.is_empty() || !self.new_failed_steps.is_empty()
    }
}

/// Compares `after` with an explicit `before` run of the same files.
/// `renamed` maps a file's path before the edit to its path after, so a
/// moved file's findings are compared with themselves. A baseline from the
/// cache holds only the asked files' findings, so then only those are
/// compared; a baseline from a check also compares the other files a step
/// reported.
pub(crate) fn between(before: &Report, after: &Report, renamed: &[(String, String)]) -> Delta {
    let moved = |f: &Finding| {
        let mut f = f.clone();
        if let Some((_, to)) = renamed.iter().find(|(from, _)| *from == f.file) {
            f.file.clone_from(to);
        }
        f
    };
    let asked_only = before.source == "cache";
    let in_scope = |f: &Finding| !asked_only || after.files.contains(&f.file);
    let earlier: Vec<Finding> = before.findings.iter().map(moved).filter(in_scope).collect();
    let later: Vec<Finding> = after
        .findings
        .iter()
        .filter(|f| in_scope(f))
        .cloned()
        .collect();
    let comparison = compare(&earlier, &later);
    let failed_before: Vec<&str> = before.failed_steps().map(|s| s.name.as_str()).collect();
    Delta {
        new: comparison.new,
        new_failed_steps: after
            .failed_steps()
            .filter(|s| s.findings == 0 && !failed_before.contains(&s.name.as_str()))
            .cloned()
            .collect(),
        kept: comparison.kept,
        fixed: comparison.fixed,
        no_baseline: Vec::new(),
        unknown: Vec::new(),
    }
}

/// Compares `after` with what the cache last saw of each file's other
/// version. `unchanged` says whether a file matches HEAD.
pub(crate) fn against_cache(
    root: &Path,
    cache: &Cache,
    after: &Report,
    unchanged: impl Fn(&str) -> bool,
) -> Delta {
    let mut before = Vec::new();
    let mut failed_before: Vec<String> = Vec::new();
    let mut no_baseline = Vec::new();
    for file in &after.files {
        let Some(now) = hash(root, file) else {
            continue;
        };
        if let Some(seen) = cache.before(file, &now) {
            before.extend(seen.findings.iter().cloned());
            failed_before.extend(seen.failed_steps.iter().cloned());
        } else if unchanged(file) {
            // Everything in a file the edit did not touch was there already.
            before.extend(after.findings.iter().filter(|f| &f.file == file).cloned());
            failed_before.extend(after.failed_steps().map(|s| s.name.clone()));
        } else {
            no_baseline.push(file.clone());
        }
    }
    // Findings in files the check was not asked about (a step such as
    // Clippy reports the whole crate) have no baseline in the cache, which
    // keeps only the asked files', so they are left out rather than counted
    // as new.
    let (unknown, known): (Vec<Finding>, Vec<Finding>) = after
        .findings
        .iter()
        .filter(|f| after.files.contains(&f.file))
        .cloned()
        .partition(|f| no_baseline.contains(&f.file));
    let comparison = compare(&before, &known);
    let new_failed_steps = if no_baseline.is_empty() {
        after
            .failed_steps()
            .filter(|s| s.findings == 0 && !failed_before.contains(&s.name))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    Delta {
        new: comparison.new,
        new_failed_steps,
        kept: comparison.kept,
        fixed: comparison.fixed,
        no_baseline,
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(file: &str, line: u64, rule: &str, message: &str) -> Finding {
        Finding {
            file: file.into(),
            line: Some(line),
            col: Some(1),
            severity: "error".into(),
            rule: Some(rule.into()),
            message: message.into(),
            source_step: "clippy".into(),
        }
    }

    #[test]
    fn findings_that_moved_are_not_new() {
        let before = [
            f("a.rs", 10, "unused", "unused variable: `x`"),
            f("a.rs", 20, "len_zero", "length comparison to zero"),
        ];
        // Twelve lines were added above both, and one new finding appeared.
        let after = [
            f("a.rs", 22, "unused", "unused variable: `x`"),
            f("a.rs", 32, "len_zero", "length comparison to zero"),
            f("a.rs", 5, "unused", "unused variable: `y`"),
        ];
        let c = compare(&before, &after);
        assert_eq!(c.new, [f("a.rs", 5, "unused", "unused variable: `y`")]);
        assert_eq!((c.kept, c.fixed), (2, 0));
    }

    #[test]
    fn a_new_finding_above_an_old_one_does_not_take_its_match() {
        let before = [f("a.rs", 10, "todo", "TODO left in")];
        // A new TODO at line 3, and the old one moved from 10 to 12.
        let after = [
            f("a.rs", 3, "todo", "TODO left in"),
            f("a.rs", 12, "todo", "TODO left in"),
        ];
        assert_eq!(
            compare(&before, &after).new,
            [f("a.rs", 3, "todo", "TODO left in")]
        );
    }

    #[test]
    fn a_reworded_finding_nearby_is_the_same_one() {
        let before = [f("a.rs", 10, "E0308", "expected `u8`, found `&str`")];
        let after = [f("a.rs", 11, "E0308", "expected `u8`, found `String`")];
        assert!(compare(&before, &after).new.is_empty());
        // Far away, it is another.
        let far = [f("a.rs", 40, "E0308", "expected `u8`, found `String`")];
        assert_eq!(compare(&before, &far).new.len(), 1);
    }

    #[test]
    fn duplicates_are_counted_one_for_one() {
        let before = [f("a.rs", 3, "r", "m")];
        let after = [f("a.rs", 3, "r", "m"), f("a.rs", 30, "r", "m")];
        let c = compare(&before, &after);
        assert_eq!(c.new, [f("a.rs", 30, "r", "m")]);
        // The same finding in another file is new.
        assert_eq!(compare(&before, &[f("b.rs", 3, "r", "m")]).new.len(), 1);
        // A message that differs only in a number is the same.
        let shifted = compare(
            &[f("a.rs", 3, "r", "defined at line 9")],
            &[f("a.rs", 80, "r", "defined at line 86")],
        );
        assert!(shifted.new.is_empty());
    }

    #[test]
    fn the_cache_keeps_the_last_two_versions() {
        let seen = |hash: &str, n: u64| Seen {
            hash: hash.into(),
            findings: vec![f("a.rs", n, "r", "m")],
            failed_steps: Vec::new(),
            at: n,
        };
        let mut cache = Cache::default();
        cache.record(vec![("a.rs".into(), seen("h1", 1))]);
        assert!(cache.before("a.rs", "h1").is_none());
        assert_eq!(cache.before("a.rs", "h2").unwrap().hash, "h1");
        cache.record(vec![("a.rs".into(), seen("h2", 2))]);
        // Checking h2 again still compares with h1.
        assert_eq!(cache.before("a.rs", "h2").unwrap().hash, "h1");
        assert_eq!(cache.at("a.rs", "h2").unwrap().findings[0].line, Some(2));
        cache.record(vec![("a.rs".into(), seen("h2", 3))]);
        assert_eq!(cache.before("a.rs", "h2").unwrap().hash, "h1");
    }

    #[test]
    fn other_files_compare_only_against_a_baseline_that_has_them() {
        let report = |files: &[&str], findings: Vec<Finding>, source: &str| {
            let mut r = Report::empty(
                super::super::Status::Findings,
                files.iter().map(|f| f.to_string()).collect(),
            );
            r.findings = findings;
            r.source = source.into();
            r
        };
        let elsewhere = f("lib.rs", 9, "r", "old");
        // From the cache: only the asked file's findings were kept, so the
        // one in lib.rs is not new.
        let before = report(&["a.rs"], vec![], "cache");
        let after = report(&["a.rs"], vec![elsewhere.clone()], "hk");
        assert!(between(&before, &after, &[]).new.is_empty());
        // From a check: lib.rs was reported then too, so it compares.
        let before = report(&["a.rs"], vec![elsewhere.clone()], "hk");
        let d = between(&before, &after, &[]);
        assert!(d.new.is_empty());
        assert_eq!(d.kept, 1);
        // A moved file's findings follow it.
        let before = report(&["old.rs"], vec![f("old.rs", 3, "r", "m")], "hk");
        let after = report(&["new.rs"], vec![f("new.rs", 3, "r", "m")], "hk");
        let renamed = [("old.rs".to_string(), "new.rs".to_string())];
        assert!(between(&before, &after, &renamed).new.is_empty());
        assert_eq!(between(&before, &after, &[]).new.len(), 1);
        // The cache's delta leaves out files it was not asked about.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        let after = report(&["a.rs"], vec![elsewhere], "hk");
        let d = against_cache(dir.path(), &Cache::default(), &after, |_| true);
        assert!(d.new.is_empty() && d.unknown.is_empty());
    }

    #[test]
    fn a_file_with_no_earlier_version_has_no_baseline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();
        let mut after = Report::empty(
            super::super::Status::Findings,
            vec!["a.rs".into(), "b.rs".into()],
        );
        after.findings = vec![f("a.rs", 1, "r", "m"), f("b.rs", 1, "r", "m")];
        let delta = against_cache(dir.path(), &Cache::default(), &after, |file| file == "b.rs");
        assert!(delta.new.is_empty());
        assert_eq!(delta.no_baseline, ["a.rs"]);
        assert_eq!(delta.unknown.len(), 1);
        assert_eq!(delta.kept, 1);
    }
}
