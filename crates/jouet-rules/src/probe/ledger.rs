//! `jouet.ci.last_run()`: the last test, lint or build run the hooks
//! recorded in the ledger (`<state dir>/ledger/ledger.jsonl`), and
//! `jouet.session.fired()`: how often a rule already spoke up to an agent in
//! a session.
//!
//! The ledger is written by `jouet hook`; without one there is no last run.
//! Each post-tool-use entry carries a `record:<kind>` decision for a run a
//! rule recorded, with its `scope` (`full` or `partial`), the `tree` the
//! run saw, and `repo`, the key the hook gives the work tree it ran in. A
//! run whose `repo` is another work tree's is passed over; entries without
//! one, and every entry when this work tree has no key, match. Lines written
//! before jouet recorded a scope read as partial, with no tree.

use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of the end of the ledger is read.
const TAIL: u64 = 256 * 1024;

/// The newest recorded run of `kind` (any kind when None) and `scope` (any
/// scope when None) in the work tree whose key is `here`.
pub(crate) fn last_run(
    dir: &Path,
    kind: Option<&str>,
    scope: Option<&str>,
    here: Option<&str>,
) -> Option<Value> {
    let mut file = std::fs::File::open(dir.join("ledger.jsonl")).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        // The read started mid-line.
        lines.remove(0);
    }
    for line in lines.iter().rev() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // A `test-run` is a `jouet test` run, recorded by itself.
        if entry["event"] != "post-tool-use" && entry["event"] != "test-run" {
            continue;
        }
        let Some((rule, recorded, recorded_scope)) = record(&entry, kind, scope) else {
            continue;
        };
        if let (Some(theirs), Some(ours)) = (entry["repo"].as_str(), here)
            && theirs != ours
        {
            continue;
        }
        let exit_code = entry["exit_code"].as_i64();
        let mut run = json!({
            "kind": recorded,
            "rule": rule,
            "ok": exit_code == Some(0),
            "scope": recorded_scope,
        });
        for key in [
            "tree",
            "exit_code",
            "shape",
            "ts",
            "duration_ms",
            "session",
            "harness",
        ] {
            if !entry[key].is_null() {
                run[key] = entry[key].clone();
            }
        }
        return Some(run);
    }
    None
}

/// How many earlier tool calls by `agent` in `session` (None for the
/// session's own agent, not its subagents) the rule `rule` denied or warned
/// about, in the end of the ledger. The hook writes ids with only their
/// letters, digits, `-` and `_` (the binary's `ledger::redact_id`), so
/// they are compared the same way.
pub(crate) fn fired(dir: &Path, rule: &str, session: &str, agent: Option<&str>) -> Option<u64> {
    let redact = |id: &str| -> String {
        id.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .take(128)
            .collect()
    };
    let session = redact(session);
    let agent = agent.map(redact);
    let mut file = std::fs::File::open(dir.join("ledger.jsonl")).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        // The read started mid-line.
        lines.next();
    }
    let count = lines
        .filter(|line| line.contains(session.as_str()) && line.contains(rule))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["event"] == "pre-tool-use" && entry["session"] == session.as_str())
        .filter(|entry| entry["agent"].as_str() == agent.as_deref())
        .filter(|entry| {
            entry["decisions"].as_array().is_some_and(|ds| {
                ds.iter()
                    .any(|d| d["rule"] == rule && (d["action"] == "deny" || d["action"] == "warn"))
            })
        })
        .count();
    Some(count as u64)
}

/// The rule, kind and scope of the entry's first `record:<kind>` decision
/// that matches `kind` and `scope`.
fn record<'a>(
    entry: &'a Value,
    kind: Option<&str>,
    scope: Option<&str>,
) -> Option<(&'a str, &'a str, &'a str)> {
    entry["decisions"].as_array()?.iter().find_map(|d| {
        let recorded = d["action"].as_str()?.strip_prefix("record:")?;
        let recorded_scope = if d["scope"] == "full" {
            "full"
        } else {
            "partial"
        };
        let wanted =
            kind.is_none_or(|k| k == recorded) && scope.is_none_or(|s| s == recorded_scope);
        wanted.then(|| Some((d["rule"].as_str()?, recorded, recorded_scope)))?
    })
}
