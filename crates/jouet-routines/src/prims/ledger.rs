//! `ledger`: what jouet's hooks recorded, read from the end of
//! `<state dir>/ledger/ledger.jsonl`. Only entries from this work tree are
//! read unless `all_repos` says otherwise; an entry carries command shapes,
//! never contents.
//!
//! - `ledger.entries{event, since, limit, all_repos}`: newest first. `since`
//!   is a duration ago such as `2h`.
//! - `ledger.last_run(kind)`: the newest test, lint or build run a rule
//!   recorded, as `{ kind, ok, exit_code, shape, ts }`, or nil.

use super::{field, primitive};
use crate::Fail;
use crate::capability::Capability::LedgerRead;
use crate::runtime::{Ctx, to_lua};
use jouet_output::exit::Exit;
use mlua::{Lua, Table};
use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

/// How much of the end of the ledger is read.
const TAIL: u64 = 1024 * 1024;
/// Entries `ledger.entries` returns unless told otherwise.
const DEFAULT_LIMIT: i64 = 100;

/// The entries at the end of the ledger, oldest first.
fn read(dir: &Path) -> Vec<Value> {
    let Ok(mut file) = std::fs::File::open(dir.join("ledger.jsonl")) else {
        return Vec::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(TAIL);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    // A read that starts mid-file starts mid-line.
    text.lines()
        .skip(usize::from(start > 0))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// This work tree's entries, newest first, or a test's recording.
fn entries(ctx: &Ctx, all_repos: bool) -> Vec<Value> {
    let world = ctx.world.borrow();
    let mut entries = match &world.recorded {
        Some(recorded) => recorded.ledger.clone(),
        None => world.ledger.as_deref().map(read).unwrap_or_default(),
    };
    if !all_repos && let Some(here) = &world.repo_key {
        entries.retain(|e| e["repo"].as_str().is_none_or(|theirs| theirs == here));
    }
    entries.reverse();
    entries
}

/// The kind a post-tool-use entry recorded, as `record:<kind>`.
fn recorded_kind(entry: &Value) -> Option<String> {
    entry["decisions"].as_array()?.iter().find_map(|d| {
        d["action"]
            .as_str()?
            .strip_prefix("record:")
            .map(str::to_string)
    })
}

pub(super) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let ledger = lua.create_table()?;
    ledger.set(
        "entries",
        primitive(
            lua,
            ctx,
            Some(LedgerRead),
            "ledger.entries",
            |lua, ctx, opts: Option<Table>| {
                let what = "ledger.entries";
                let event: Option<String> = field(opts.as_ref(), "event", what)?;
                let limit: i64 = field(opts.as_ref(), "limit", what)?.unwrap_or(DEFAULT_LIMIT);
                let all_repos: bool = field(opts.as_ref(), "all_repos", what)?.unwrap_or(false);
                let since = match field::<String>(opts.as_ref(), "since", what)? {
                    Some(text) => {
                        let ago = jouet_output::time::parse_duration(&text).map_err(|e| {
                            Fail::new(Exit::Usage, format!("{what}: `since`: {e}")).raise()
                        })?;
                        let ago = i64::try_from(ago.as_secs()).unwrap_or(i64::MAX);
                        Some(jouet_output::time::now().saturating_sub(ago))
                    }
                    None => None,
                };
                let found: Vec<Value> = entries(ctx, all_repos)
                    .into_iter()
                    .filter(|e| event.as_deref().is_none_or(|ev| e["event"] == ev))
                    .filter(|e| since.is_none_or(|t| e["ts"].as_i64().is_some_and(|ts| ts >= t)))
                    .take(usize::try_from(limit).unwrap_or(0))
                    .collect();
                to_lua(lua, &Value::Array(found))
            },
        )?,
    )?;
    ledger.set(
        "last_run",
        primitive(
            lua,
            ctx,
            Some(LedgerRead),
            "ledger.last_run",
            |lua, ctx, kind: Option<String>| {
                if let Some(kind) = kind.as_deref()
                    && !["test", "lint", "build"].contains(&kind)
                {
                    return Err(mlua::Error::runtime(format!(
                        "ledger.last_run: kind is \"test\", \"lint\" or \"build\", not `{kind}`"
                    )));
                }
                let run = entries(ctx, false).into_iter().find_map(|entry| {
                    if entry["event"] != "post-tool-use" {
                        return None;
                    }
                    let recorded = recorded_kind(&entry)?;
                    if kind.as_deref().is_some_and(|k| k != recorded) {
                        return None;
                    }
                    let exit_code = entry["exit_code"].as_i64();
                    Some(json!({
                        "kind": recorded,
                        "ok": exit_code == Some(0),
                        "exit_code": exit_code,
                        "shape": entry["shape"],
                        "ts": entry["ts"],
                    }))
                });
                to_lua(lua, &run.unwrap_or(Value::Null))
            },
        )?,
    )?;
    globals.set("ledger", ledger)?;
    Ok(())
}
