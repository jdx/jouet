//! `jouet stack`: a stack of PRs, each built on the one below, checked by
//! ancestry and by content. Which branches form the stack comes from
//! gh-stack's metadata when it tracks the branch, else from the PRs' bases
//! on GitHub; what git says about them is jouet-git's.

use super::Output;
use crate::{StackArgs, StackCmd, StackStatusArgs, StackSyncArgs};
use jouet_git::Git;
use jouet_git::stack::{self, Layer, LayerState, Stack};
use jouet_github::Client;
use jouet_github::pr::target::{self, Repo};
use jouet_output::exit::Exit;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write as _;

/// How many PRs a stack may have before jouet stops following bases.
const MAX_LAYERS: usize = 20;

pub(crate) fn run(args: StackArgs, out: &Output) -> Exit {
    match args.cmd {
        StackCmd::Status(a) => status(&a, out),
        StackCmd::Sync(a) => sync(&a, out),
    }
}

/// A PR, as the stack shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pr {
    pub number: u64,
    pub url: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: String,
    pub base: String,
    pub head: String,
    pub head_sha: String,
}

/// The stack around a branch: its layers bottom first, with their PRs.
pub(crate) struct Found {
    /// `gh-stack` or `github`.
    pub source: &'static str,
    pub stack: Stack,
    pub prs: Vec<Option<Pr>>,
    /// Something the caller should know, such as a branch with several PRs
    /// built on it.
    pub notes: Vec<String>,
}

/// Why a stack could not be found: the exit and the message.
pub(crate) type Failure = (Exit, String);

const PR_FIELDS: &str = "number url state isDraft baseRefName headRefName headRefOid";

fn pr_from(node: &Value) -> Option<Pr> {
    let state = match node["state"].as_str()? {
        "OPEN" if node["isDraft"] == true => "draft",
        "OPEN" => "open",
        "MERGED" => "merged",
        _ => "closed",
    };
    Some(Pr {
        number: node["number"].as_u64()?,
        url: node["url"].as_str()?.into(),
        state: state.into(),
        base: node["baseRefName"].as_str()?.into(),
        head: node["headRefName"].as_str()?.into(),
        head_sha: node["headRefOid"].as_str().unwrap_or_default().into(),
    })
}

/// The PR for a branch: its open one, else the newest.
fn pick(nodes: &Value) -> Option<Pr> {
    let prs: Vec<Pr> = nodes.as_array()?.iter().filter_map(pr_from).collect();
    prs.iter()
        .find(|p| p.state == "open" || p.state == "draft")
        .or_else(|| prs.first())
        .cloned()
}

/// What one round trip finds: the default branch, the PR for each head
/// asked about, and the open PRs based on each base asked about.
type Answer = (String, Vec<Option<Pr>>, Vec<Vec<Pr>>);

/// One round trip: the default branch, the PRs whose head is each of
/// `heads`, and the open PRs based on each of `bases`.
fn query(
    client: &mut Client,
    repo: &Repo,
    heads: &[String],
    bases: &[String],
) -> Result<Answer, Failure> {
    let mut decls = vec!["$owner: String!".to_string(), "$name: String!".to_string()];
    let mut fields = vec!["defaultBranchRef { name }".to_string()];
    let mut vars = json!({ "owner": repo.owner, "name": repo.name });
    for (i, head) in heads.iter().enumerate() {
        decls.push(format!("$h{i}: String!"));
        fields.push(format!(
            "h{i}: pullRequests(headRefName: $h{i}, first: 10, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ {PR_FIELDS} }} }}"
        ));
        vars[format!("h{i}")] = head.as_str().into();
    }
    for (i, base) in bases.iter().enumerate() {
        decls.push(format!("$b{i}: String!"));
        fields.push(format!(
            "b{i}: pullRequests(baseRefName: $b{i}, states: [OPEN], first: 10, orderBy: {{field: CREATED_AT, direction: ASC}}) {{ nodes {{ {PR_FIELDS} }} }}"
        ));
        vars[format!("b{i}")] = base.as_str().into();
    }
    let q = format!(
        "query({}) {{ repository(owner: $owner, name: $name) {{ {} }} }}",
        decls.join(", "),
        fields.join(" ")
    );
    let data = client.graphql(&q, &vars).map_err(|e| (Exit::Remote, e))?;
    let r = &data["repository"];
    if r.is_null() {
        return Err((Exit::NotFound, format!("no GitHub repository {repo}")));
    }
    let default = r["defaultBranchRef"]["name"]
        .as_str()
        .unwrap_or("main")
        .to_string();
    let by_head = (0..heads.len())
        .map(|i| pick(&r[format!("h{i}")]["nodes"]))
        .collect();
    let by_base = (0..bases.len())
        .map(|i| {
            r[format!("b{i}")]["nodes"]
                .as_array()
                .map(|n| n.iter().filter_map(pr_from).collect())
                .unwrap_or_default()
        })
        .collect();
    Ok((default, by_head, by_base))
}

/// The stack holding `branch`: gh-stack's, else the chain of PR bases.
pub(crate) fn find(
    git: &Git,
    client: &mut Client,
    repo: &Repo,
    branch: &str,
) -> Result<Found, Failure> {
    let remote = stack::remote(git);
    if let Some(gh) = stack::gh_stack(git, branch) {
        let heads: Vec<String> = gh.branches.iter().map(|(b, _)| b.clone()).collect();
        let mut notes = Vec::new();
        // gh-stack already names the layers, so without GitHub only which
        // PRs merged is unknown.
        let prs = match query(client, repo, &heads, &[]) {
            Ok((_, prs, _)) => prs,
            Err((Exit::Remote, message)) => {
                notes.push(format!(
                    "GitHub could not be reached, so merged PRs are not known: {message}"
                ));
                vec![None; heads.len()]
            }
            Err(e) => return Err(e),
        };
        let stack = layers(&gh.trunk, &remote, &heads, &prs);
        return Ok(Found {
            source: "gh-stack",
            stack,
            prs,
            notes,
        });
    }
    let mut notes = Vec::new();
    let (default, mut found, children) =
        query(client, repo, &[branch.to_string()], &[branch.to_string()])?;
    let Some(pr) = found.pop().flatten() else {
        return Err((
            Exit::NotFound,
            format!(
                "`{branch}` has no PR and gh-stack does not track it, so jouet cannot tell which stack it is in"
            ),
        ));
    };
    // Down: each PR's base, until the default branch.
    let mut below: Vec<Pr> = Vec::new();
    let mut base = pr.base.clone();
    while base != default && below.len() < MAX_LAYERS {
        let (_, mut prs, _) = query(client, repo, &[base.clone()], &[])?;
        let Some(p) = prs.pop().flatten() else {
            notes.push(format!(
                "`{base}` has no PR; the stack is shown from the layer above it"
            ));
            break;
        };
        base = p.base.clone();
        below.push(p);
    }
    below.reverse();
    let trunk = below.first().map_or(pr.base.clone(), |p| p.base.clone());
    // Up: the one open PR built on each layer, while there is just one.
    let mut above: Vec<Pr> = Vec::new();
    let mut next = children.into_iter().next().unwrap_or_default();
    while above.len() < MAX_LAYERS {
        match next.len() {
            0 => break,
            1 => {
                let child = next.remove(0);
                let (_, _, mut c) = query(client, repo, &[], std::slice::from_ref(&child.head))?;
                next = c.pop().unwrap_or_default();
                above.push(child);
            }
            n => {
                let on = above.last().map_or(&pr.head, |p| &p.head);
                notes.push(format!(
                    "{n} open PRs are built on `{on}`; the stack is shown up to it"
                ));
                break;
            }
        }
    }
    let prs: Vec<Option<Pr>> = below
        .into_iter()
        .chain([pr])
        .chain(above)
        .map(Some)
        .collect();
    let heads: Vec<String> = prs.iter().flatten().map(|p| p.head.clone()).collect();
    let stack = layers(&trunk, &remote, &heads, &prs);
    Ok(Found {
        source: "github",
        stack,
        prs,
        notes,
    })
}

/// The layers of branches `heads`, bottom first, each built on the one
/// below. A merged PR has left the stack, so the layer above it needs the
/// trunk.
fn layers(trunk: &str, remote: &str, heads: &[String], prs: &[Option<Pr>]) -> Stack {
    let merged = |i: usize| prs[i].as_ref().is_some_and(|p| p.state == "merged");
    let layers = heads
        .iter()
        .enumerate()
        .map(|(i, head)| Layer {
            branch: head.clone(),
            parent: i
                .checked_sub(1)
                .map_or(trunk.to_string(), |j| heads[j].clone()),
            merged: merged(i),
            parent_merged: i.checked_sub(1).is_some_and(merged),
        })
        .collect();
    Stack {
        trunk: trunk.into(),
        remote: remote.into(),
        layers,
    }
}

/// One layer of `stack status`.
#[derive(Serialize)]
struct Row<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pr: Option<&'a Pr>,
    #[serde(flatten)]
    state: &'a LayerState,
    /// What needs doing, worst first; absent when the layer is in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    needs: Vec<&'static str>,
}

#[derive(Serialize)]
struct Report<'a> {
    source: &'static str,
    trunk: &'a str,
    remote: &'a str,
    layers: Vec<Row<'a>>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    notes: &'a [String],
}

/// The repository, the client and the branch a command works on.
pub(crate) fn setup(branch: Option<&str>) -> Result<(Git, Client, Repo, String), Failure> {
    let git = Git::here();
    git.toplevel().map_err(|e| (e.exit, e.message))?;
    let repo = target::repo_from_git().ok_or((
        Exit::Precondition,
        "no GitHub remote in this checkout".to_string(),
    ))?;
    if let (Some(host), Some(forge)) = (&repo.host, repo.forge()) {
        return Err((
            Exit::Precondition,
            format!(
                "jouet stack works with GitHub PRs only so far, and this checkout's remote is {} on {host}",
                forge.name()
            ),
        ));
    }
    let branch = match branch {
        Some(b) => b.to_string(),
        None => target::current_branch().ok_or((
            Exit::Precondition,
            "HEAD is detached: name the branch, as in `jouet stack status BRANCH`".to_string(),
        ))?,
    };
    Ok((git, Client::from_env(), repo, branch))
}

fn status(args: &StackStatusArgs, out: &Output) -> Exit {
    let (git, mut client, repo, branch) = match setup(args.branch.as_deref()) {
        Ok(s) => s,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let found = match find(&git, &mut client, &repo, &branch) {
        Ok(f) => f,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let states = match stack::inspect(&git, &found.stack) {
        Ok(s) => s,
        Err(e) => return out.error(e.exit, &e.message),
    };
    let rows: Vec<Row> = states
        .iter()
        .zip(&found.prs)
        .map(|(state, pr)| Row {
            pr: pr.as_ref(),
            state,
            needs: state.needs(),
        })
        .collect();
    let attention = rows.iter().any(|r| !r.needs.is_empty());
    let report = Report {
        source: found.source,
        trunk: &found.stack.trunk,
        remote: &found.stack.remote,
        layers: rows,
        notes: &found.notes,
    };
    out.result(&report, || text(&report, &branch));
    if attention { Exit::Failed } else { Exit::Ok }
}

fn text(report: &Report, current: &str) -> String {
    let mut s = String::new();
    let chain: Vec<String> = std::iter::once(report.trunk.to_string())
        .chain(report.layers.iter().map(|r| match r.pr {
            Some(pr) => format!("#{}", pr.number),
            None => r.state.branch.clone(),
        }))
        .collect();
    let _ = writeln!(
        s,
        "STACK {} (from {})",
        chain.join(" ← "),
        if report.source == "gh-stack" {
            "gh-stack"
        } else {
            "PR bases on GitHub"
        }
    );
    let width = report
        .layers
        .iter()
        .map(|r| r.state.branch.len())
        .max()
        .unwrap_or(0);
    let numbers: Vec<String> = report
        .layers
        .iter()
        .map(|r| r.pr.map(|p| format!("#{} ", p.number)).unwrap_or_default())
        .collect();
    let nw = numbers.iter().map(String::len).max().unwrap_or(0);
    for (r, number) in report.layers.iter().zip(&numbers) {
        let st = r.state;
        let mark = if st.branch == current { "*" } else { " " };
        let _ = write!(s, "{mark} {number:>nw$}{:width$}  ", st.branch);
        if st.merged {
            s.push_str("merged\n");
            continue;
        }
        let mut words = Vec::new();
        for need in &r.needs {
            words.push(match *need {
                "missing_branch" => "no local or remote branch".to_string(),
                "parent_changes_missing" => {
                    let files: Vec<String> = st
                        .content
                        .iter()
                        .map(|f| match &f.commit {
                            Some(commit) => format!("{} (undoes {commit})", f.path),
                            None => format!("{} ({})", f.path, f.problem.describe()),
                        })
                        .collect();
                    format!(
                        "has {}'s commits but not its changes to {}",
                        st.parent,
                        files.join(", ")
                    )
                }
                "behind_remote" => format!("{} behind {}/{}", st.behind, report.remote, st.branch),
                "needs_trunk" => format!(
                    "its base merged: needs {}/{} merged in",
                    report.remote, report.trunk
                ),
                "needs_parent" if st.parent == report.trunk => {
                    format!("needs {}/{} merged in", report.remote, report.trunk)
                }
                "needs_parent" => format!("needs {} merged in", st.parent),
                other => other.to_string(),
            });
        }
        if words.is_empty() {
            words.push(format!("contains {}", st.parent));
        }
        if st.unpushed > 0 {
            words.push(format!("{} unpushed", st.unpushed));
        }
        let _ = writeln!(s, "{}", words.join("; "));
        for f in &st.content {
            let _ = writeln!(
                s,
                "    restore it with `git checkout {} -- {}` and commit, unless the {} is meant",
                st.parent,
                f.path,
                match f.problem {
                    stack::Problem::Reverted => "revert",
                    _ => "difference",
                }
            );
        }
    }
    let mergeable = report.layers.iter().any(|r| {
        r.needs
            .iter()
            .any(|n| matches!(*n, "needs_parent" | "needs_trunk"))
    });
    if mergeable {
        s.push_str("next: `jouet stack sync --dry-run` shows the merges that bring each layer up to date; `jouet stack sync` makes them\n");
    }
    for note in report.notes {
        let _ = writeln!(s, "note: {note}");
    }
    s
}

fn sync(args: &StackSyncArgs, out: &Output) -> Exit {
    let (git, mut client, repo, branch) = match setup(args.branch.as_deref()) {
        Ok(s) => s,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let found = match find(&git, &mut client, &repo, &branch) {
        Ok(f) => f,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let opts = stack::sync::Options {
        dry_run: args.dry_run,
        push: args.push,
        fetch: !args.no_fetch,
    };
    let report = stack::sync::sync(&git, &found.stack, opts);
    let chain = std::iter::once(found.stack.trunk.clone())
        .chain(
            found
                .stack
                .layers
                .iter()
                .zip(&found.prs)
                .map(|(l, pr)| match pr {
                    Some(pr) => format!("#{}", pr.number),
                    None => l.branch.clone(),
                }),
        )
        .collect::<Vec<_>>()
        .join(" ← ");
    let doc = json!({
        "source": found.source,
        "trunk": found.stack.trunk,
        "report": report,
        "notes": found.notes,
    });
    out.result(&doc, || sync_text(&report, &chain, &found));
    report.exit()
}

fn sync_text(report: &stack::sync::Report, chain: &str, found: &Found) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{} {chain}{}",
        if report.dry_run { "PLAN" } else { "SYNC" },
        if report.dry_run {
            " (dry run: nothing fetched, merged or pushed)"
        } else {
            ""
        }
    );
    let width = found
        .stack
        .layers
        .iter()
        .map(|l| l.branch.len())
        .max()
        .unwrap_or(0);
    for step in &report.steps {
        let from = step.from.as_deref().unwrap_or("");
        let what = match step.action {
            "up_to_date" => "up to date".to_string(),
            "skipped_merged" => "merged; skipped".to_string(),
            "would_merge" => format!("would merge {from}"),
            "merged" => format!(
                "merged {from} ({})",
                step.commit.as_deref().map_or("", |c| &c[..c.len().min(7)])
            ),
            other => other.to_string(),
        };
        let created = if step.created {
            if report.dry_run {
                "; would create the local branch from the remote's"
            } else {
                "; local branch created from the remote's"
            }
        } else {
            ""
        };
        let _ = writeln!(s, "  {:width$}  {what}{created}", step.branch);
    }
    if let Some(stopped) = &report.stopped {
        let _ = writeln!(s, "STOPPED at {}: {}", stopped.branch, stopped.message);
        if !stopped.files.is_empty() {
            let _ = writeln!(s, "  files: {}", stopped.files.join(", "));
        }
        let _ = writeln!(s, "  next: {}", stopped.next);
    }
    for push in &report.pushed {
        match &push.error {
            None => {
                let _ = writeln!(s, "PUSHED {}", push.branch);
            }
            Some(e) => {
                let _ = writeln!(s, "PUSH FAILED {}: {e}", push.branch);
            }
        }
    }
    for note in &found.notes {
        let _ = writeln!(s, "note: {note}");
    }
    s
}
