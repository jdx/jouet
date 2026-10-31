//! `github`: jouet-github's client, which finds a token that works,
//! retries 5xx responses and secondary rate limits, and says when an
//! exhausted rate limit resets.
//!
//! - `github.repo()`: the repository the git remote names.
//! - `github.graphql(query, vars)` and `github.mutate(query, vars)`.
//! - `github.get(path, {etag})`: one REST read; `github.paginate(path)`
//!   follows `Link` headers and joins the pages' lists.
//! - `github.request(method, path, body)`: a REST write.
//! - `github.prs{...}`: the PRs `pr status` and `pr next` select, with their
//!   status and attention codes.

use super::{field, primitive, strings};
use crate::Fail;
use crate::capability::Capability::{GithubRead, GithubWrite};
use crate::runtime::{Ctx, from_lua, to_lua};
use jouet_github::pr::select::{self, Checkout, Selection};
use jouet_github::pr::target::Repo;
use jouet_github::{Failure, Method, Rest, next_page};
use jouet_output::exit::Exit;
use mlua::{Lua, Table, Value};
use serde_json::json;
use std::rc::Rc;
use std::time::Duration;

/// The most pages `github.paginate` reads unless told otherwise.
const MAX_PAGES: i64 = 10;

fn remote(message: impl Into<String>) -> mlua::Error {
    Fail::new(Exit::Remote, message).raise()
}

fn failure(f: Failure) -> mlua::Error {
    remote(f.message())
}

/// A REST response as a table: `status`, `etag`, `next` (the next page's
/// URL) and `body`, parsed when it is JSON.
fn rest_table(lua: &Lua, rest: &Rest) -> mlua::Result<Value> {
    let body = serde_json::from_str::<serde_json::Value>(&rest.body)
        .unwrap_or_else(|_| rest.body.clone().into());
    let mut value = json!({ "status": rest.status, "body": body });
    if let Some(etag) = &rest.etag {
        value["etag"] = etag.as_str().into();
    }
    if let Some(next) = rest.link.as_deref().and_then(next_page) {
        value["next"] = next.into();
    }
    to_lua(lua, &value)
}

/// A test's checkout: the fixture's `git.repo` and `git.state`'s branch.
struct Recorded {
    repo: Option<Repo>,
    branch: Option<String>,
}

impl Checkout for Recorded {
    fn repo(&self) -> Option<Repo> {
        self.repo.clone()
    }

    fn branch(&self) -> Option<String> {
        self.branch.clone()
    }
}

pub(super) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let github = lua.create_table()?;
    github.set(
        "repo",
        primitive(lua, ctx, Some(GithubRead), "github.repo", |lua, ctx, ()| {
            let recorded = ctx
                .world
                .borrow()
                .recorded
                .as_ref()
                .map(|r| r.git.get("repo").cloned().unwrap_or_default());
            let repo = match recorded {
                Some(repo) => repo,
                // `github.repo` is the GitHub repository, so a GitLab or
                // Forgejo remote is none.
                None => {
                    match jouet_github::pr::target::repo_from_git().filter(|r| r.host.is_none()) {
                        Some(r) => {
                            json!({ "owner": r.owner, "name": r.name, "repo": r.to_string() })
                        }
                        None => serde_json::Value::Null,
                    }
                }
            };
            to_lua(lua, &repo)
        })?,
    )?;
    github.set(
        "graphql",
        primitive(
            lua,
            ctx,
            Some(GithubRead),
            "github.graphql",
            |lua, ctx, (query, vars): (String, Option<Value>)| {
                let vars = vars
                    .map(|v| from_lua(lua, &v))
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                let data = ctx.client().graphql(&query, &vars).map_err(remote)?;
                to_lua(lua, &data)
            },
        )?,
    )?;
    github.set(
        "mutate",
        primitive(
            lua,
            ctx,
            Some(GithubWrite),
            "github.mutate",
            |lua, ctx, (query, vars): (String, Option<Value>)| {
                let vars = vars
                    .map(|v| from_lua(lua, &v))
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                let name = query
                    .split_whitespace()
                    .take_while(|w| !w.starts_with('{') && !w.starts_with('('))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !ctx.write(format!("GraphQL {name}")) {
                    return to_lua(lua, &json!({ "dry_run": true }));
                }
                let data = ctx.client().mutate(&query, &vars).map_err(remote)?;
                to_lua(lua, &data)
            },
        )?,
    )?;
    github.set(
        "get",
        primitive(
            lua,
            ctx,
            Some(GithubRead),
            "github.get",
            |lua, ctx, (path, opts): (String, Option<Table>)| {
                let etag: Option<String> = field(opts.as_ref(), "etag", "github.get")?;
                let rest = ctx.client().get(&path, etag.as_deref()).map_err(failure)?;
                rest_table(lua, &rest)
            },
        )?,
    )?;
    github.set(
        "paginate",
        primitive(
            lua,
            ctx,
            Some(GithubRead),
            "github.paginate",
            |lua, ctx, (path, opts): (String, Option<Table>)| {
                let max: i64 =
                    field(opts.as_ref(), "max_pages", "github.paginate")?.unwrap_or(MAX_PAGES);
                let mut items = Vec::new();
                let mut url = Some(path);
                let mut pages = 0;
                while let Some(next) = url.take() {
                    if pages >= max {
                        break;
                    }
                    pages += 1;
                    let rest = ctx.client().get(&next, None).map_err(failure)?;
                    if rest.status != 200 {
                        return Err(remote(format!(
                            "GitHub returned {} for {next}",
                            rest.status
                        )));
                    }
                    let body: serde_json::Value =
                        serde_json::from_str(&rest.body).map_err(|e| {
                            remote(format!("GitHub sent a page jouet could not read: {e}"))
                        })?;
                    // Most lists are the body; a few wrap it, as `items` or
                    // `workflow_runs` do.
                    let page = match body {
                        serde_json::Value::Array(page) => page,
                        serde_json::Value::Object(fields) => fields
                            .into_iter()
                            .find_map(|(_, v)| match v {
                                serde_json::Value::Array(page) => Some(page),
                                _ => None,
                            })
                            .unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    items.extend(page);
                    url = rest.link.as_deref().and_then(next_page);
                }
                to_lua(lua, &serde_json::Value::Array(items))
            },
        )?,
    )?;
    github.set(
        "request",
        primitive(
            lua,
            ctx,
            Some(GithubWrite),
            "github.request",
            |lua, ctx, (method, path, body): (String, String, Option<Value>)| {
                let Some(m) = Method::parse(&method) else {
                    return Err(mlua::Error::runtime(format!(
                        "github.request: the method is POST, PUT, PATCH or DELETE (use github.get to read), not `{method}`"
                    )));
                };
                let body = body.map(|b| from_lua(lua, &b)).transpose()?.unwrap_or_else(|| json!({}));
                if !ctx.write(format!("{} {path}", method.to_ascii_uppercase())) {
                    return to_lua(lua, &json!({ "dry_run": true }));
                }
                let rest = ctx.client().write(m, &path, &body).map_err(failure)?;
                rest_table(lua, &rest)
            },
        )?,
    )?;
    github.set(
        "prs",
        primitive(
            lua,
            ctx,
            Some(GithubRead),
            "github.prs",
            |lua, ctx, opts: Option<Table>| {
                let what = "github.prs";
                let prs = strings(opts.as_ref(), "prs", what)?;
                let repo = strings(opts.as_ref(), "repo", what)?;
                let search: Option<String> = field(opts.as_ref(), "search", what)?;
                let mine: bool = field(opts.as_ref(), "mine", what)?.unwrap_or(false);
                let settle = match field::<f64>(opts.as_ref(), "settle", what)? {
                    Some(s) if s >= 0.0 => Duration::from_secs_f64(s),
                    Some(_) => {
                        return Err(mlua::Error::runtime(
                            "github.prs: `settle` cannot be negative",
                        ));
                    }
                    None => select::DEFAULT_SETTLE,
                };
                let recorded = ctx.world.borrow().recorded.as_ref().map(|r| Recorded {
                    repo: r
                        .git
                        .get("repo")
                        .and_then(|v| v["repo"].as_str())
                        .and_then(Repo::parse),
                    branch: r
                        .git
                        .get("state")
                        .and_then(|v| v["branch"].as_str())
                        .map(str::to_string),
                });
                let selection = Selection {
                    prs: &prs,
                    mine,
                    search: search.as_deref(),
                    repo: &repo,
                    checkout: match &recorded {
                        Some(recorded) => recorded,
                        None => &select::Git,
                    },
                };
                let lookups =
                    select::lookups(&selection).map_err(|(exit, e)| Fail::new(exit, e).raise())?;
                let report = select::fetch(&mut ctx.client(), lookups, settle).map_err(remote)?;
                let value = serde_json::to_value(&report).map_err(mlua::Error::runtime)?;
                to_lua(lua, &value)
            },
        )?,
    )?;
    globals.set("github", github)?;
    Ok(())
}
