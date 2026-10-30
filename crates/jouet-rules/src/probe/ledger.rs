//! `jouet.ci.last_run()`: the last test, lint or build run the hooks
//! recorded in the ledger (`<state dir>/ledger/ledger.jsonl`).
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
        if entry["event"] != "post-tool-use" {
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
