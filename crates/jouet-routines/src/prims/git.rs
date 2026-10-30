//! `git`: read-only views of the work tree, through the `git` on PATH, so
//! config and attributes behave as they do for the agent's own git. It
//! never takes an optional lock. Writes are left to `jouet commit`.
//!
//! - `git.state()`: branch, upstream, ahead/behind and dirtiness.
//! - `git.diff{base, staged, paths, stat}`: a diff's text, or with `stat`
//!   its files and line counts.
//! - `git.log{range, max, paths}`: commits, newest first.
//! - `git.refs(pattern)`: refs and the commits they point at.
//! - `git.rev(name)`: the full SHA a name points at, or nil.

use super::{field, primitive, strings};
use crate::Fail;
use crate::capability::Capability::GitRead;
use crate::runtime::{Ctx, to_lua};
use jouet_output::exit::Exit;
use mlua::{Lua, Table};
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use std::rc::Rc;

/// Commits `git.log` lists unless told otherwise.
const DEFAULT_MAX: i64 = 20;

/// Runs git in the work tree, or answers from a test's recording under
/// `key`.
fn git(ctx: &Ctx, key: &str, args: &[String]) -> mlua::Result<Result<String, String>> {
    let world = ctx.world.borrow();
    if let Some(recorded) = &world.recorded {
        return match recorded.git.get(key) {
            Some(Value::String(text)) => Ok(Ok(text.clone())),
            Some(_) => Ok(Err(format!("the fixture's git.{key} must be text"))),
            None => Err(Fail::new(
                Exit::Failed,
                format!("the fixture records no answer for git.{key}"),
            )
            .raise()),
        };
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(&world.root)
        .arg("--no-optional-locks")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Fail::new(Exit::Failed, format!("running git: {e}")).raise())?;
    if !out.status.success() {
        return Ok(Err(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(Ok(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// The recorded JSON for a primitive whose answer is structured, if the run
/// is a test's.
fn recorded(ctx: &Ctx, key: &str) -> mlua::Result<Option<Value>> {
    let world = ctx.world.borrow();
    let Some(recorded) = &world.recorded else {
        return Ok(None);
    };
    match recorded.git.get(key) {
        Some(value) => Ok(Some(value.clone())),
        None => Err(Fail::new(
            Exit::Failed,
            format!("the fixture records no answer for git.{key}"),
        )
        .raise()),
    }
}

fn failed(what: &str, message: String) -> mlua::Error {
    Fail::new(Exit::Failed, format!("{what}: {message}")).raise()
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

pub(super) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.set(
        "state",
        primitive(lua, ctx, Some(GitRead), "git.state", |lua, ctx, ()| {
            if let Some(value) = recorded(ctx, "state")? {
                return to_lua(lua, &value);
            }
            let out = git(
                ctx,
                "state",
                &words(&["status", "--porcelain=v2", "--branch"]),
            )?
            .map_err(|e| failed("git.state", e))?;
            to_lua(lua, &jouet_git::status::parse(&out))
        })?,
    )?;
    table.set(
        "diff",
        primitive(
            lua,
            ctx,
            Some(GitRead),
            "git.diff",
            |lua, ctx, opts: Option<Table>| {
                let what = "git.diff";
                let base: Option<String> = field(opts.as_ref(), "base", what)?;
                let staged: bool = field(opts.as_ref(), "staged", what)?.unwrap_or(false);
                let stat: bool = field(opts.as_ref(), "stat", what)?.unwrap_or(false);
                let paths = strings(opts.as_ref(), "paths", what)?;
                let mut args = words(&["diff", "--no-color", "--no-ext-diff"]);
                if stat {
                    args.push("--numstat".into());
                }
                if staged {
                    args.push("--cached".into());
                }
                if let Some(base) = base {
                    if base.starts_with('-') {
                        return Err(failed(
                            what,
                            format!("`base` cannot start with `-`: `{base}`"),
                        ));
                    }
                    args.push(base);
                }
                args.push("--".into());
                args.extend(paths);
                let out = git(ctx, "diff", &args)?.map_err(|e| failed(what, e))?;
                if !stat {
                    return to_lua(lua, &out.into());
                }
                let files: Vec<Value> = out
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, '\t');
                    let (added, deleted, path) = (parts.next()?, parts.next()?, parts.next()?);
                    // A binary file's counts are `-`.
                    let count = |n: &str| n.parse::<u64>().ok().map_or(Value::Null, Into::into);
                    Some(json!({ "path": path, "added": count(added), "deleted": count(deleted) }))
                })
                .collect();
                to_lua(lua, &Value::Array(files))
            },
        )?,
    )?;
    table.set(
        "log",
        primitive(
            lua,
            ctx,
            Some(GitRead),
            "git.log",
            |lua, ctx, opts: Option<Table>| {
                let what = "git.log";
                if let Some(value) = recorded(ctx, "log")? {
                    return to_lua(lua, &value);
                }
                let range: Option<String> = field(opts.as_ref(), "range", what)?;
                let max: i64 = field(opts.as_ref(), "max", what)?.unwrap_or(DEFAULT_MAX);
                let paths = strings(opts.as_ref(), "paths", what)?;
                let mut args =
                    words(&["log", "--no-color", "-z", "--format=%H%x1f%an%x1f%aI%x1f%s"]);
                args.push(format!("--max-count={max}"));
                if let Some(range) = range {
                    if range.starts_with('-') {
                        return Err(failed(
                            what,
                            format!("`range` cannot start with `-`: `{range}`"),
                        ));
                    }
                    args.push(range);
                }
                args.push("--".into());
                args.extend(paths);
                let out = git(ctx, "log", &args)?.map_err(|e| failed(what, e))?;
                let commits: Vec<Value> = out
                    .split('\0')
                    .filter(|c| !c.trim().is_empty())
                    .filter_map(|c| {
                        let mut f = c.trim_start_matches('\n').split('\x1f');
                        Some(json!({
                            "sha": f.next()?,
                            "author": f.next()?,
                            "date": f.next()?,
                            "subject": f.next()?,
                        }))
                    })
                    .collect();
                to_lua(lua, &Value::Array(commits))
            },
        )?,
    )?;
    table.set(
        "refs",
        primitive(
            lua,
            ctx,
            Some(GitRead),
            "git.refs",
            |lua, ctx, pattern: Option<String>| {
                if let Some(value) = recorded(ctx, "refs")? {
                    return to_lua(lua, &value);
                }
                let mut args = words(&["for-each-ref", "--format=%(refname)%09%(objectname)"]);
                args.extend(pattern);
                let out = git(ctx, "refs", &args)?.map_err(|e| failed("git.refs", e))?;
                let refs: Vec<Value> = out
                    .lines()
                    .filter_map(|line| {
                        let (name, sha) = line.split_once('\t')?;
                        Some(json!({ "name": name, "sha": sha }))
                    })
                    .collect();
                to_lua(lua, &Value::Array(refs))
            },
        )?,
    )?;
    table.set(
        "rev",
        primitive(
            lua,
            ctx,
            Some(GitRead),
            "git.rev",
            |_, ctx, name: String| {
                if let Some(value) = recorded(ctx, "rev")? {
                    return Ok(value[&name].as_str().map(str::to_string));
                }
                let args = vec![
                    "rev-parse".to_string(),
                    "--verify".into(),
                    "--quiet".into(),
                    "--end-of-options".into(),
                    format!("{name}^{{commit}}"),
                ];
                Ok(git(ctx, "rev", &args)?.ok().map(|s| s.trim().to_string()))
            },
        )?,
    )?;
    globals.set("git", table)?;
    Ok(())
}
