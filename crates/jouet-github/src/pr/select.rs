//! The PRs a command names: arguments, `--mine`, `--search` and `--repo`,
//! turned into lookups and fetched. `jouet pr status`, `jouet pr next` and
//! the routines' `github.prs` share it, so they always select the same PRs.

use super::status::{self, Incomplete, Lookup, PrStatus};
use super::target::{self, Arg, Repo, Target};
use crate::Client;
use jouet_output::time;
use serde::Serialize;
use std::fmt::Write as _;
use std::time::Duration;

/// How long to wait for GitHub to compute an unknown mergeable state when
/// `--settle` is not given.
pub const DEFAULT_SETTLE: Duration = Duration::from_secs(20);

/// The PRs a command names.
pub struct Selection<'a> {
    pub prs: &'a [String],
    pub mine: bool,
    pub search: Option<&'a str>,
    pub repo: &'a [String],
    /// Where the checkout's repository and branch come from.
    pub checkout: &'a dyn Checkout,
}

/// The checkout a bare number or branch, and the current branch's PR, are
/// read against.
pub trait Checkout {
    /// The GitHub repository the remotes name.
    fn repo(&self) -> Option<Repo>;
    /// The branch checked out.
    fn branch(&self) -> Option<String>;
}

/// The checkout in this process's directory, through git.
pub struct Git;

impl Checkout for Git {
    fn repo(&self) -> Option<Repo> {
        target::repo_from_git()
    }

    fn branch(&self) -> Option<String> {
        target::current_branch()
    }
}

/// The PRs found, in the order they were named, and what was not.
#[derive(Debug, Serialize)]
pub struct Report {
    pub prs: Vec<PrStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<Incomplete>,
}

/// Fetches the PRs the lookups name, waits up to `settle` for unknown
/// mergeability, and derives their attention codes.
pub fn fetch(
    client: &mut Client,
    lookups: Vec<Lookup>,
    settle: Duration,
) -> Result<Report, String> {
    let mut fetched = status::fetch(client, lookups)?;
    status::settle(client, &mut fetched.prs, settle)?;
    let now = time::now();
    Ok(Report {
        prs: fetched.prs.into_iter().map(|p| p.finish(now)).collect(),
        not_found: fetched.not_found,
        incomplete: fetched.incomplete,
    })
}

/// Turns the arguments and selectors into lookups, in the order given.
pub fn lookups(args: &Selection<'_>) -> Result<Vec<Lookup>, String> {
    let repos = args
        .repo
        .iter()
        .map(|r| Repo::parse(r).ok_or_else(|| format!("--repo takes OWNER/NAME, not `{r}`")))
        .collect::<Result<Vec<_>, _>>()?;
    let git_repo = std::cell::OnceCell::new();
    let git_repo = || git_repo.get_or_init(|| args.checkout.repo()).clone();
    let repo_for = |arg: &str| match repos.as_slice() {
        [repo] => Ok(repo.clone()),
        [] => git_repo().ok_or_else(|| {
            format!("no --repo given and no GitHub remote here, so `{arg}` names no repository")
        }),
        _ => Err(format!(
            "`{arg}` is ambiguous with {} --repo values; write it as OWNER/REPO#N",
            repos.len()
        )),
    };
    let mut lookups = Vec::new();
    for arg in args.prs {
        let target = match Arg::parse(arg) {
            Arg::Number(n) => Target::Number(repo_for(arg)?, n),
            Arg::Branch(branch) => Target::Branch(repo_for(arg)?, branch),
            Arg::InRepo(repo, n) => Target::Number(repo, n),
        };
        lookups.push(Lookup::Pr(target));
    }
    // Searches cover every --repo, else this checkout's repository, else all
    // of GitHub.
    let scope: Vec<Repo> = if repos.is_empty() {
        git_repo().into_iter().collect()
    } else {
        repos.clone()
    };
    if args.mine {
        lookups.push(search("is:open author:@me", &scope));
    }
    if let Some(query) = args.search {
        let scoped = ["repo:", "org:", "user:"]
            .iter()
            .any(|q| query.split_whitespace().any(|w| w.starts_with(q)));
        lookups.push(search(query, if scoped { &[] } else { &scope }));
    }
    if lookups.is_empty() {
        let branch = args
            .checkout
            .branch()
            .ok_or("not on a branch: name a PR, or use --mine or --search")?;
        let repo = repo_for(&branch)?;
        lookups.push(Lookup::Pr(Target::Branch(repo, branch)));
    }
    Ok(lookups)
}

fn search(query: &str, repos: &[Repo]) -> Lookup {
    let mut query = format!("is:pr {query}");
    for repo in repos {
        let _ = write!(query, " repo:{repo}");
    }
    Lookup::Search {
        query,
        after: None,
        listed: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_are_scoped_to_repos() {
        let repos = [
            Repo::parse("jdx/mise").unwrap(),
            Repo::parse("jdx/hk").unwrap(),
        ];
        assert_eq!(
            search("is:open author:@me", &repos),
            Lookup::Search {
                query: "is:pr is:open author:@me repo:jdx/mise repo:jdx/hk".into(),
                after: None,
                listed: 0,
            }
        );
    }
}
