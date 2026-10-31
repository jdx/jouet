//! `edit`: jouet-edit's two phases. `edit.plan(spec, opts)` applies a spec
//! to an in-memory view of its files and reports each edit, writing
//! nothing; `edit.apply(spec, opts)` writes the files only when every edit
//! succeeded, and restores them if a write fails part way. The spec is the
//! text `jouet edit` reads (search/replace blocks, a patch, or JSON), and
//! `opts` takes `format`, `diff`, and `expect_hash`, a table of paths and
//! the SHA-256 hashes they must have.
//!
//! Both return `jouet edit --json`'s report; `report.ok` is false when an
//! edit failed, in which case nothing was written.

use super::{field, primitive};
use crate::Fail;
use crate::capability::Capability::{FsRead, FsWrite};
use crate::runtime::{Ctx, to_lua};
use jouet_edit::spec::{self, Format};
use jouet_edit::{Options, Plan};
use jouet_output::exit::Exit;
use mlua::{Lua, Table};
use std::rc::Rc;

/// Plans `text` with the options a routine gave.
fn plan(text: &str, opts: Option<&Table>, what: &str) -> mlua::Result<Plan> {
    let usage = |message: String| Fail::new(Exit::Usage, format!("{what}: {message}")).raise();
    let format = match field::<String>(opts, "format", what)? {
        Some(name) => Format::parse(&name)
            .ok_or_else(|| usage(format!("`format` is blocks, patch or json, not `{name}`")))?,
        None => Format::detect(text)
            .ok_or_else(|| usage("cannot tell the spec's format; pass `format`".into()))?,
    };
    let spec = spec::parse(text, format).map_err(|e| usage(e.to_string()))?;
    let mut expect_hashes = Vec::new();
    if let Some(hashes) = field::<Table>(opts, "expect_hash", what)? {
        for pair in hashes.pairs::<String, String>() {
            expect_hashes.push(pair.map_err(|e| usage(format!("`expect_hash`: {e}")))?);
        }
    }
    let options = Options {
        expect_hashes,
        diff: field(opts, "diff", what)?.unwrap_or(false),
        // Routines have no grammars: a symbol edit is refused.
        symbols: None,
    };
    jouet_edit::plan(&spec, &options)
        .map_err(|e| Fail::new(Exit::Failed, format!("{what}: reading files: {e}")).raise())
}

fn report(lua: &Lua, plan: &Plan) -> mlua::Result<mlua::Value> {
    let mut value = serde_json::to_value(&plan.report).map_err(mlua::Error::runtime)?;
    value["ok"] = plan.report.ok().into();
    to_lua(lua, &value)
}

pub(super) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let edit = lua.create_table()?;
    edit.set(
        "plan",
        primitive(
            lua,
            ctx,
            Some(FsRead),
            "edit.plan",
            |lua, _, (text, opts): (String, Option<Table>)| {
                let mut plan = plan(&text, opts.as_ref(), "edit.plan")?;
                plan.report.dry_run = true;
                report(lua, &plan)
            },
        )?,
    )?;
    edit.set(
        "apply",
        primitive(lua, ctx, Some(FsWrite), "edit.apply", |lua, ctx, (text, opts): (String, Option<Table>)| {
            let mut plan = plan(&text, opts.as_ref(), "edit.apply")?;
            if !plan.report.ok() {
                return report(lua, &plan);
            }
            let files: Vec<String> = plan
                .report
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect();
            // Under `joue test` the files are the developer's own, so an
            // edit is only ever shown.
            let description = format!("edit {}", files.join(", "));
            let acting = if ctx.options.testing {
                ctx.note(description);
                false
            } else {
                ctx.write(description)
            };
            if !acting {
                plan.report.dry_run = true;
                return report(lua, &plan);
            }
            jouet_edit::write(&mut plan).map_err(|e| {
                Fail::new(
                    Exit::Failed,
                    format!("edit.apply: writing failed, and the files already written were restored: {e}"),
                )
                .raise()
            })?;
            report(lua, &plan)
        })?,
    )?;
    globals.set("edit", edit)?;
    Ok(())
}
