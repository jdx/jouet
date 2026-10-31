//! The host trait: what jouet's PR commands ask of a forge, answered by
//! GitHub, GitLab or Forgejo in the same model types, so `pr status`,
//! `pr threads` and the waits print the same output and exit the same way
//! whichever forge a repository is on.
//!
//! A forge that has no equivalent of something says so through
//! [`Host::lacks`] before anything is sent, and the command exits 4 with
//! that reason rather than approximating it.

use crate::forge::{self, Forge};
use crate::pr::status::{self, FetchResult, Lookup};
use crate::pr::target::{Repo, Target};
use crate::pr::threads::{self, PrThreads, Pushed, Thread};
use crate::{Client, forgejo, gitlab};
use serde_json::json;
use std::fmt::Write as _;

/// What a forge may lack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// `--search` with GitHub's search syntax.
    Search,
    /// Resolving and unresolving review threads.
    Resolve,
    /// Merging and updating a PR's branch: `pr land`.
    Land,
    /// The runs and jobs of one CI run: `wait run`.
    Runs,
    /// The history `ci explain` judges a failure by.
    Explain,
    /// Which checks branch protection requires: `wait ci --required`.
    RequiredChecks,
}

pub trait Host {
    fn forge(&self) -> Forge;

    /// Why the forge cannot do `feature`, or `None` when it can.
    fn lacks(&mut self, feature: Feature) -> Option<String>;

    /// The PRs the lookups name, in the order named, as `pr status`
    /// reports them before attention codes are derived.
    fn fetch(&mut self, lookups: Vec<Lookup>) -> Result<FetchResult, String>;

    /// A PR's review threads, and with `extra` its reviews and conversation
    /// comments. `None` when there is no such PR.
    fn threads(
        &mut self,
        target: &Target,
        extra: bool,
        bots: &[String],
    ) -> Result<Option<PrThreads>, String>;

    /// When `sha` (a prefix) was pushed to the PR.
    fn pushed_at(&mut self, pr: &PrThreads, sha: &str) -> Result<Pushed, threads::Lookup>;

    /// Files at commit `oid`, as `(path, text)`; a file that cannot be read
    /// is left out.
    fn files(
        &mut self,
        pr: &PrThreads,
        oid: &str,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String>;

    /// The commit the PR's diff is taken against.
    fn merge_base(&mut self, pr: &PrThreads) -> Result<String, String>;

    /// Whether `head` is on the PR's branch, for a head this clone cannot
    /// place.
    fn on_branch(&mut self, pr: &PrThreads, head: &str) -> Result<bool, String>;

    /// Posts `body` as a reply in `thread`, returning the reply's URL.
    fn reply(&mut self, pr: &PrThreads, thread: &Thread, body: &str) -> Result<String, String>;

    /// Resolves or unresolves `thread`.
    fn set_resolved(
        &mut self,
        pr: &PrThreads,
        thread: &Thread,
        resolved: bool,
    ) -> Result<(), String>;
}

/// The host a repository is on: `github` for GitHub, else a new client for
/// its GitLab or Forgejo host. `Err` names a host jouet does not know.
pub fn connect(repo: &Repo) -> Result<Box<dyn Host>, String> {
    match (repo.forge(), &repo.host) {
        (Some(Forge::GitLab), Some(host)) => Ok(Box::new(gitlab::GitLab::new(host))),
        (Some(Forge::Forgejo), Some(host)) => Ok(Box::new(forgejo::Forgejo::new(host))),
        (_, Some(host)) if repo.forge().is_none() => Err(forge::unknown(host)),
        _ => Ok(Box::new(Client::from_env())),
    }
}

/// Runs `f` with the host `repo` is on: `github` for a GitHub repository,
/// else a new client for its forge.
pub fn with<T>(
    repo: &Repo,
    github: &mut Client,
    f: impl FnOnce(&mut dyn Host) -> Result<T, String>,
) -> Result<T, String> {
    if repo.host.is_none() {
        return f(github);
    }
    f(connect(repo)?.as_mut())
}

/// Fetches lookups that may be on several forges: GitHub's through
/// `github`, every other host's through its own client, in the order each
/// host was first named.
pub fn fetch(github: &mut Client, lookups: Vec<Lookup>) -> Result<FetchResult, String> {
    let mut groups: Vec<(Option<Repo>, Vec<Lookup>)> = Vec::new();
    for lookup in lookups {
        let repo = lookup.repo().filter(|r| r.host.is_some()).cloned();
        let host = repo.as_ref().and_then(|r| r.host.clone());
        match groups
            .iter_mut()
            .find(|(r, _)| r.as_ref().and_then(|r| r.host.clone()) == host)
        {
            Some((_, group)) => group.push(lookup),
            None => groups.push((repo, vec![lookup])),
        }
    }
    let mut result = FetchResult::default();
    for (repo, group) in groups {
        let fetched = match repo {
            None => status::fetch(github, group)?,
            Some(repo) => connect(&repo)?.fetch(group)?,
        };
        result.prs.extend(fetched.prs);
        result.not_found.extend(fetched.not_found);
        result.incomplete.extend(fetched.incomplete);
    }
    Ok(result)
}

const REPLY: &str = "mutation($id: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $id, body: $body}) { comment { url } }
}";
const RESOLVE: &str =
    "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }";
const UNRESOLVE: &str = "mutation($id: ID!) { unresolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }";

impl Host for Client {
    fn forge(&self) -> Forge {
        Forge::GitHub
    }

    fn lacks(&mut self, _feature: Feature) -> Option<String> {
        None
    }

    fn fetch(&mut self, lookups: Vec<Lookup>) -> Result<FetchResult, String> {
        status::fetch(self, lookups)
    }

    fn threads(
        &mut self,
        target: &Target,
        extra: bool,
        bots: &[String],
    ) -> Result<Option<PrThreads>, String> {
        threads::fetch(self, target, extra, bots)
    }

    fn pushed_at(&mut self, pr: &PrThreads, sha: &str) -> Result<Pushed, threads::Lookup> {
        threads::pushed_at(self, pr, sha)
    }

    /// Reads every file in one GraphQL query.
    fn files(
        &mut self,
        pr: &PrThreads,
        oid: &str,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        let mut q = String::from("query($o: String!, $r: String!");
        let mut body = String::new();
        let mut vars = json!({ "o": pr.repo.owner, "r": pr.repo.name });
        for (i, path) in paths.iter().enumerate() {
            let _ = write!(q, ", $e{i}: String!");
            let _ = write!(
                body,
                " f{i}: object(expression: $e{i}) {{ ... on Blob {{ text }} }}"
            );
            vars[format!("e{i}")] = format!("{oid}:{path}").into();
        }
        let _ = write!(q, ") {{ repository(owner: $o, name: $r) {{{body} }} }}");
        let data = self.graphql(&q, &vars)?;
        Ok(paths
            .iter()
            .enumerate()
            .filter_map(|(i, path)| {
                let text = data["repository"][format!("f{i}")]["text"].as_str()?;
                Some((path.clone(), text.to_string()))
            })
            .collect())
    }

    fn merge_base(&mut self, pr: &PrThreads) -> Result<String, String> {
        let path = format!(
            "/repos/{}/compare/{}...{}?per_page=1",
            pr.repo, pr.base_oid, pr.head_oid
        );
        let compare = self
            .rest_get(&path)?
            .ok_or("GitHub could not compare the PR's base and head")?;
        compare["merge_base_commit"]["sha"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "GitHub's comparison names no merge base".into())
    }

    fn on_branch(&mut self, pr: &PrThreads, head: &str) -> Result<bool, String> {
        let repo = pr.head_repo.as_ref().unwrap_or(&pr.repo);
        let path = format!("/repos/{repo}/compare/{head}...{}", pr.head_oid);
        let compare = self.rest_get(&path)?;
        Ok(compare.is_some_and(|c| matches!(c["status"].as_str(), Some("ahead" | "identical"))))
    }

    fn reply(&mut self, _pr: &PrThreads, thread: &Thread, body: &str) -> Result<String, String> {
        let data = self.mutate(REPLY, &json!({ "id": thread.node_id, "body": body }))?;
        data["addPullRequestReviewThreadReply"]["comment"]["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "GitHub did not post the reply".into())
    }

    fn set_resolved(
        &mut self,
        _pr: &PrThreads,
        thread: &Thread,
        resolved: bool,
    ) -> Result<(), String> {
        let (query, field, verb) = if resolved {
            (RESOLVE, "resolveReviewThread", "resolve")
        } else {
            (UNRESOLVE, "unresolveReviewThread", "unresolve")
        };
        let data = self.mutate(query, &json!({ "id": thread.node_id }))?;
        if data[field]["thread"]["isResolved"].as_bool() != Some(resolved) {
            return Err(format!("GitHub did not {verb} the thread"));
        }
        Ok(())
    }
}
