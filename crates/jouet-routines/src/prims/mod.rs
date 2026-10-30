//! The primitives: thin wrappers over jouet's crates, one global table each.
//!
//! - `github`: GraphQL and REST through jouet-github's client (auth,
//!   retries, ETags, rate limits), pagination, and `github.prs`, the PR
//!   selection and status `pr status` and `pr next` use.
//! - `git`: the work tree's state, diffs, history and refs. Read-only.
//! - `wait`: `jouet wait`'s engine, for CI, PRs, runs, files, logs, ports
//!   and URLs.
//! - `jouet`: jouet's own PR commands, such as `pr land`.
//! - `edit`: jouet-edit's all-or-nothing plan and write.
//! - `ledger`: the hooks' ledger.
//! - `mise`: the resolved config and task list, and running tasks.
//!
//! Every primitive checks the routine's capabilities first, and runs
//! outside its Luau budget.

mod edit;
mod git;
mod github;
mod jouet;
mod ledger;
mod mise;

pub use jouet::COMMANDS;
pub use mise::test_task;

use crate::capability::Capability;
use crate::runtime::Ctx;
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, Table};
use std::rc::Rc;

pub(crate) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    github::install(lua, globals, ctx)?;
    git::install(lua, globals, ctx)?;
    jouet::install(lua, globals, ctx)?;
    edit::install(lua, globals, ctx)?;
    ledger::install(lua, globals, ctx)?;
    mise::install(lua, globals, ctx)?;
    Ok(())
}

/// A primitive: refused without `capability`, and run outside the budget.
fn primitive<A, R>(
    lua: &Lua,
    ctx: &Rc<Ctx>,
    capability: Option<Capability>,
    name: impl Into<String>,
    f: impl Fn(&Lua, &Ctx, A) -> mlua::Result<R> + 'static,
) -> mlua::Result<Function>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
{
    let ctx = ctx.clone();
    let name = name.into();
    lua.create_function(move |lua, args: A| {
        if let Some(capability) = capability {
            ctx.need(capability, &name)?;
        }
        ctx.outside(|| f(lua, &ctx, args))
    })
}

/// A table's field as an optional value of the type asked for, with an
/// error naming the primitive and the field.
fn field<T: mlua::FromLua>(
    opts: Option<&Table>,
    name: &str,
    what: &str,
) -> mlua::Result<Option<T>> {
    let Some(opts) = opts else { return Ok(None) };
    opts.raw_get::<Option<T>>(name)
        .map_err(|e| mlua::Error::runtime(format!("{what}: `{name}`: {e}")))
}

/// A field that may be one string or a list of them.
fn strings(opts: Option<&Table>, name: &str, what: &str) -> mlua::Result<Vec<String>> {
    match field::<mlua::Value>(opts, name, what)? {
        None | Some(mlua::Value::Nil) => Ok(Vec::new()),
        Some(mlua::Value::String(s)) => Ok(vec![s.to_string_lossy()]),
        Some(mlua::Value::Table(t)) => t
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()
            .map_err(|e| mlua::Error::runtime(format!("{what}: `{name}`: {e}"))),
        Some(other) => Err(mlua::Error::runtime(format!(
            "{what}: `{name}` must be a string or a list of strings, not a {}",
            other.type_name()
        ))),
    }
}
