//! Forgejo and Gitea pull requests behind [`Host`], through the REST API
//! (v1), which the two share.
//!
//! The API has no review threads, only review comments: comments on the
//! same file and line make one thread, as the web UI shows them, and a
//! thread's ID is its first comment's. A reply is a new single-comment
//! review on that line. Checks are the head commit's statuses, which is
//! where Forgejo Actions reports its jobs.
//!
//! Resolving a conversation is only in Gitea's API, from 1.26
//! (`POST /repos/{owner}/{repo}/pulls/comments/{id}/resolve`); Forgejo's
//! API has no such endpoint (checked through v16), so it is refused there.

use crate::forge::Forge;
use crate::host::{Feature, Host};
use crate::pr::status::{Checks, FetchResult, Fetched, Lookup, Mergeability, PrStatus, State};
use crate::pr::target::{Repo, Target};
use crate::pr::threads::{self, Comment, Kind, PrThreads, Pushed, Side, Thread};
use crate::rest::{MAX_PAGES, Rest, encode, utc};
use crate::{Failure, Method};
use jouet_output::time;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub struct Forgejo {
    rest: Rest,
    viewer: Option<String>,
    /// What `/version` said, once asked.
    version: Option<Option<String>>,
}

impl Forgejo {
    pub fn new(host: &str) -> Self {
        Self {
            rest: Rest::new(Forge::Forgejo, host),
            viewer: None,
            version: None,
        }
    }

    /// The REST client, for the CI reads `wait` makes.
    pub fn rest(&mut self) -> &mut Rest {
        &mut self.rest
    }

    /// The PR a target names: a number as is, a branch through its open PR,
    /// else its most recently updated. `None` when there is none.
    pub fn number(&mut self, target: &Target) -> Result<Option<u64>, Failure> {
        let (repo, branch) = match target {
            Target::Number(_, n) => return Ok(Some(*n)),
            Target::Branch(repo, branch) => (repo, branch),
        };
        for state in ["open", "all"] {
            let path = format!(
                "{}/pulls?state={state}&sort=recentupdate&limit=50",
                base(repo)
            );
            let prs = self.rest.list(&path)?.unwrap_or_default();
            if let Some(n) = prs
                .iter()
                .find(|p| p["head"]["ref"].as_str() == Some(branch))
                .and_then(|p| p["number"].as_u64())
            {
                return Ok(Some(n));
            }
        }
        Ok(None)
    }

    fn viewer(&mut self) -> String {
        if self.viewer.is_none() {
            // Reading a public PR needs no token, and then there is no
            // viewer.
            let login = self
                .rest
                .json("/user")
                .ok()
                .flatten()
                .and_then(|u| u["login"].as_str().map(str::to_string));
            self.viewer = Some(login.unwrap_or_default());
        }
        self.viewer.clone().unwrap_or_default()
    }

    /// The server's version, such as `1.26.0` (Gitea) or
    /// `16.0.0+gitea-1.22.0` (Forgejo).
    fn version(&mut self) -> Option<String> {
        if self.version.is_none() {
            let v = self.rest.json("/version").ok().flatten();
            self.version = Some(v.and_then(|v| v["version"].as_str().map(str::to_string)));
        }
        self.version.clone().flatten()
    }

    fn status(&mut self, repo: &Repo, pr: &Value) -> Result<Fetched, Failure> {
        let n = pr["number"].as_u64().unwrap_or_default();
        let state = match (pr["state"].as_str(), pr["merged"].as_bool()) {
            (_, Some(true)) => State::Merged,
            (Some("closed"), _) => State::Closed,
            _ if pr["draft"].as_bool() == Some(true) => State::Draft,
            _ => State::Open,
        };
        let (mergeable, merge_state) = match pr["mergeable"].as_bool() {
            Some(true) if state == State::Draft => (Mergeability::Mergeable, "draft"),
            Some(true) => (Mergeability::Mergeable, "clean"),
            Some(false) => (Mergeability::Conflicting, "dirty"),
            None => (Mergeability::Unknown, "unknown"),
        };
        let head_oid = pr["head"]["sha"].as_str().unwrap_or_default().to_string();
        let reviews = self
            .rest
            .list(&format!("{}/pulls/{n}/reviews?limit=50", base(repo)))?
            .unwrap_or_default();
        let comments = review_comments(&mut self.rest, repo, n, &reviews)?;
        let unresolved = group(comments)
            .iter()
            .filter(|t| t[0]["resolver"].is_null())
            .count();
        let mut checks = Checks::default();
        if !head_oid.is_empty() {
            for s in statuses(&mut self.rest, repo, &head_oid)? {
                let name = s["context"].as_str().unwrap_or_default().to_string();
                match s["status"].as_str().unwrap_or_default() {
                    "success" => checks.pass += 1,
                    "failure" | "error" => {
                        checks.fail += 1;
                        checks.failing.push(name);
                    }
                    "warning" | "skipped" => checks.skipped += 1,
                    _ => checks.pending += 1,
                }
            }
        }
        let pushed_at = self
            .rest
            .list(&format!("{}/pulls/{n}/commits?limit=50", base(repo)))
            .ok()
            .flatten()
            .and_then(|c| utc(&c.last()?["commit"]["committer"]["date"]));
        Ok(Fetched::new(PrStatus {
            repo: repo.to_string(),
            number: n,
            title: pr["title"].as_str().unwrap_or_default().to_string(),
            url: pr["html_url"].as_str().unwrap_or_default().to_string(),
            state,
            base: pr["base"]["ref"].as_str().unwrap_or_default().to_string(),
            head: pr["head"]["ref"].as_str().unwrap_or_default().to_string(),
            head_sha: head_oid.chars().take(10).collect(),
            head_oid,
            forge: Some("forgejo"),
            host: repo.host.clone(),
            pushed_at,
            mergeable,
            merge_state: merge_state.to_string(),
            checks,
            review: decision(&reviews),
            unresolved_threads: u32::try_from(unresolved).unwrap_or(u32::MAX),
            auto_merge: false,
            attention: Vec::new(),
        }))
    }

    fn commits(&mut self, pr: &PrThreads) -> Result<Vec<(String, i64)>, String> {
        let list = self
            .rest
            .list(&format!(
                "{}/pulls/{}/commits?limit=50",
                base(&pr.repo),
                pr.number
            ))?
            .unwrap_or_default();
        Ok(list
            .iter()
            .map(|c| {
                let oid = c["sha"].as_str().unwrap_or_default().to_string();
                let at = c["commit"]["committer"]["date"]
                    .as_str()
                    .and_then(time::parse_rfc3339)
                    .unwrap_or(0);
                (oid, at)
            })
            .collect())
    }
}

/// A link as Forgejo Actions writes it, by path alone
/// (`/owner/repo/actions/runs/1/jobs/0`), made absolute on the host's web
/// server.
pub fn link(rest: &Rest, url: &str) -> String {
    if url.starts_with('/') {
        format!("{}{url}", rest.web_root())
    } else {
        url.to_string()
    }
}

/// `/repos/OWNER/NAME`.
pub fn base(repo: &Repo) -> String {
    format!("/repos/{}/{}", encode(&repo.owner), encode(&repo.name))
}

/// Every status on `sha`, the latest of each context, through every page.
pub fn statuses(rest: &mut Rest, repo: &Repo, sha: &str) -> Result<Vec<Value>, Failure> {
    let mut out: Vec<Value> = Vec::new();
    for page in 1..=MAX_PAGES {
        let path = format!("{}/commits/{sha}/status?limit=50&page={page}", base(repo));
        let Some(combined) = rest.json(&path)? else {
            break;
        };
        let list = combined["statuses"].as_array().cloned().unwrap_or_default();
        let total = combined["total_count"].as_u64().unwrap_or(0);
        let empty = list.is_empty();
        out.extend(list);
        if empty || out.len() as u64 >= total {
            break;
        }
    }
    Ok(out)
}

/// The review decision: changes requested if any reviewer's latest
/// standing review asks for them, else approved if one approves.
fn decision(reviews: &[Value]) -> Option<String> {
    let mut latest: BTreeMap<&str, &str> = BTreeMap::new();
    for r in reviews {
        if r["dismissed"].as_bool() == Some(true) {
            continue;
        }
        let (Some(user), Some(state)) = (r["user"]["login"].as_str(), r["state"].as_str()) else {
            continue;
        };
        if matches!(state, "APPROVED" | "REQUEST_CHANGES") {
            latest.insert(user, state);
        }
    }
    if latest.values().any(|s| *s == "REQUEST_CHANGES") {
        Some("changes_requested".into())
    } else if latest.values().any(|s| *s == "APPROVED") {
        Some("approved".into())
    } else {
        None
    }
}

/// The line comments of every review that has any.
fn review_comments(
    rest: &mut Rest,
    repo: &Repo,
    n: u64,
    reviews: &[Value],
) -> Result<Vec<Value>, Failure> {
    let mut out = Vec::new();
    for r in reviews {
        let (Some(id), Some(count)) = (r["id"].as_u64(), r["comments_count"].as_u64()) else {
            continue;
        };
        if count == 0 {
            continue;
        }
        let path = format!("{}/pulls/{n}/reviews/{id}/comments", base(repo));
        out.extend(rest.list(&path)?.unwrap_or_default());
    }
    Ok(out)
}

/// The side and line of a review comment: `position` on the head's side,
/// `original_position` on the base's.
fn place(c: &Value) -> (Side, Option<u32>) {
    let line = |k: &str| {
        c[k].as_u64()
            .filter(|l| *l > 0)
            .and_then(|l| u32::try_from(l).ok())
    };
    match line("position") {
        Some(l) => (Side::Right, Some(l)),
        None => (Side::Left, line("original_position")),
    }
}

/// Review comments grouped into threads by file, side and line, oldest
/// first in each and threads in the order they started.
fn group(mut comments: Vec<Value>) -> Vec<Vec<Value>> {
    comments.sort_by_key(|c| {
        (
            c["created_at"]
                .as_str()
                .and_then(time::parse_rfc3339)
                .unwrap_or(0),
            c["id"].as_u64().unwrap_or(0),
        )
    });
    let mut threads: Vec<Vec<Value>> = Vec::new();
    for c in comments {
        let key = |c: &Value| (c["path"].as_str().map(str::to_string), place(c));
        match threads.iter_mut().find(|t| key(&t[0]) == key(&c)) {
            Some(t) => t.push(c),
            None => threads.push(vec![c]),
        }
    }
    threads
}

fn comment(c: &Value) -> Comment {
    let created_at = utc(&c["created_at"]).unwrap_or_default();
    Comment {
        id: c["id"].as_u64().unwrap_or_default(),
        author: c["user"]["login"].as_str().unwrap_or("ghost").to_string(),
        created: time::parse_utc(&created_at).unwrap_or(0),
        created_at,
        body: c["body"].as_str().unwrap_or_default().to_string(),
        body_truncated: false,
        suggestion: None,
        url: c["html_url"].as_str().unwrap_or_default().to_string(),
    }
}

fn other(kind: Kind, c: Comment, state: Option<String>, bots: &[String]) -> Thread {
    Thread {
        id: c.id,
        node_id: String::new(),
        kind,
        path: None,
        line: None,
        start_line: None,
        original_line: None,
        side: Side::Right,
        outdated: false,
        resolved: false,
        is_bot: threads::is_bot(&c.author, bots),
        author: c.author.clone(),
        state,
        url: c.url.clone(),
        comments: vec![c],
        context: None,
    }
}

/// The major and minor of a Gitea version, or `None` for Forgejo (whose
/// version names the Gitea it forked from after a `+`) or one unreadable.
fn gitea_version(version: &str) -> Option<(u32, u32)> {
    if version.contains("+gitea") || version.contains("forgejo") {
        return None;
    }
    let mut parts = version.trim_start_matches('v').split(['.', '+', '-']);
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    // Forgejo numbers its releases from 7 up; Gitea is still 1.x.
    (major == 1).then_some((major, minor))
}

impl Host for Forgejo {
    fn forge(&self) -> Forge {
        Forge::Forgejo
    }

    fn lacks(&mut self, feature: Feature) -> Option<String> {
        let host = self.rest.host().to_string();
        match feature {
            Feature::Search => Some(
                "--search takes GitHub's search syntax, which Forgejo has no equivalent of".into(),
            ),
            Feature::Runs => Some(format!(
                "jouet reads Forgejo Actions through the commit statuses it posts, and {host} gives no run of its own; wait on the PR with `jouet wait ci` instead"
            )),
            Feature::Explain => Some(
                "`ci explain` judges a failure by GitHub Actions' run history, which Forgejo's API does not give; use `jouet wait ci` for the failed checks".into(),
            ),
            Feature::Resolve => match self.version() {
                Some(v) => match gitea_version(&v) {
                    Some((1, minor)) if minor >= 26 => None,
                    Some(_) => Some(format!(
                        "{host} runs Gitea {v}, whose API cannot resolve review conversations (Gitea 1.26 added it)"
                    )),
                    None => Some(format!(
                        "{host} runs Forgejo {v}, whose API cannot resolve review conversations (none through Forgejo 16 can)"
                    )),
                },
                // An unknown version is tried; the server says if it cannot.
                None => None,
            },
            Feature::Land => Some(
                "`pr land` merges GitHub PRs only so far; merge the PR with `tea pulls merge` or on the web".into(),
            ),
        }
    }

    fn fetch(&mut self, lookups: Vec<Lookup>) -> Result<FetchResult, String> {
        let mut result = FetchResult::default();
        for lookup in lookups {
            match lookup {
                Lookup::Pr(target) => {
                    let repo = target.repo().clone();
                    let pr = match self.number(&target)? {
                        Some(n) => self.rest.json(&format!("{}/pulls/{n}", base(&repo)))?,
                        None => None,
                    };
                    match pr {
                        Some(pr) => result.prs.push(self.status(&repo, &pr)?),
                        None => result.not_found.push(target.to_string()),
                    }
                }
                Lookup::Mine(repo) => {
                    let viewer = self.viewer();
                    if viewer.is_empty() {
                        return Err(format!(
                            "--mine needs to know who you are, and {} answered no user for the token; set FORGEJO_TOKEN",
                            self.rest.host()
                        ));
                    }
                    let path = format!(
                        "{}/pulls?state=open&poster={}&limit=50",
                        base(&repo),
                        encode(&viewer)
                    );
                    for pr in self.rest.list(&path)?.unwrap_or_default() {
                        result.prs.push(self.status(&repo, &pr)?);
                    }
                }
                Lookup::Search { .. } | Lookup::Threads { .. } | Lookup::Contexts { .. } => {}
            }
        }
        Ok(result)
    }

    fn threads(
        &mut self,
        target: &Target,
        extra: bool,
        bots: &[String],
    ) -> Result<Option<PrThreads>, String> {
        let repo = target.repo().clone();
        let Some(n) = self.number(target)? else {
            return Ok(None);
        };
        let Some(raw) = self.rest.json(&format!("{}/pulls/{n}", base(&repo)))? else {
            return Ok(None);
        };
        let head_repo = raw["head"]["repo"]["full_name"]
            .as_str()
            .filter(|f| *f != repo.to_string())
            .and_then(|f| Repo::on(self.rest.host(), f));
        let viewer = self.viewer();
        let mut pr = PrThreads {
            repo: repo.clone(),
            number: n,
            node_id: String::new(),
            head_ref: raw["head"]["ref"].as_str().unwrap_or_default().to_string(),
            head_oid: raw["head"]["sha"].as_str().unwrap_or_default().to_string(),
            base_oid: raw["merge_base"].as_str().unwrap_or_default().to_string(),
            head_repo,
            viewer,
            threads: Vec::new(),
            others: Vec::new(),
            unreadable: 0,
        };
        let reviews = self
            .rest
            .list(&format!("{}/pulls/{n}/reviews?limit=50", base(&repo)))?
            .unwrap_or_default();
        for comments in group(review_comments(&mut self.rest, &repo, n, &reviews)?) {
            let first = &comments[0];
            let (side, line) = place(first);
            let mut t = other(Kind::Thread, comment(first), None, bots);
            t.path = first["path"].as_str().map(str::to_string);
            t.side = side;
            t.line = line;
            t.original_line = line;
            t.resolved = !first["resolver"].is_null();
            t.comments = comments.iter().map(comment).collect();
            pr.threads.push(t);
        }
        if extra {
            for r in &reviews {
                let body = r["body"].as_str().unwrap_or_default();
                if body.trim().is_empty() || r["state"].as_str() == Some("PENDING") {
                    continue;
                }
                let state = match r["state"].as_str().unwrap_or_default() {
                    "REQUEST_CHANGES" => "changes_requested".to_string(),
                    "COMMENT" => "commented".to_string(),
                    s => s.to_ascii_lowercase(),
                };
                let mut c = comment(r);
                c.created_at = utc(&r["submitted_at"]).unwrap_or_default();
                c.created = time::parse_utc(&c.created_at).unwrap_or(0);
                pr.others.push(other(Kind::Review, c, Some(state), bots));
            }
            let path = format!("{}/issues/{n}/comments?limit=50", base(&repo));
            for c in self.rest.list(&path)?.unwrap_or_default() {
                if c["body"].as_str().is_some_and(|b| !b.trim().is_empty()) {
                    pr.others
                        .push(other(Kind::Comment, comment(&c), None, bots));
                }
            }
            pr.others
                .sort_by_key(|t| t.comments.first().map_or(0, |c| c.created));
        }
        Ok(Some(pr))
    }

    /// Forgejo keeps no push log, so a commit's push time is its commit
    /// time, which can only be earlier.
    fn pushed_at(&mut self, pr: &PrThreads, sha: &str) -> Result<Pushed, threads::Lookup> {
        let sha = threads::sha_prefix(sha)?;
        let commits = self.commits(pr).map_err(threads::Lookup::Remote)?;
        threads::pushed_from(&commits, &[], &sha)
    }

    fn files(
        &mut self,
        pr: &PrThreads,
        oid: &str,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for path in paths {
            let segments: Vec<String> = path.split('/').map(encode).collect();
            let url = format!("{}/raw/{}?ref={oid}", base(&pr.repo), segments.join("/"));
            let resp = self.rest.get(&url, None)?;
            if resp.status == 200 {
                out.push((path.clone(), resp.body));
            }
        }
        Ok(out)
    }

    fn merge_base(&mut self, pr: &PrThreads) -> Result<String, String> {
        if pr.base_oid.is_empty() {
            return Err("Forgejo sent the PR without its merge base".into());
        }
        Ok(pr.base_oid.clone())
    }

    fn on_branch(&mut self, pr: &PrThreads, head: &str) -> Result<bool, String> {
        Ok(self.commits(pr)?.iter().any(|(oid, _)| oid == head))
    }

    fn reply(&mut self, pr: &PrThreads, thread: &Thread, body: &str) -> Result<String, String> {
        let path = thread.path.as_deref().ok_or("the thread is on no file")?;
        let line = thread
            .original_line
            .ok_or("the thread is on no line of the file")?;
        let mut comment = json!({ "path": path, "body": body });
        let key = match thread.side {
            Side::Right => "new_position",
            Side::Left => "old_position",
        };
        comment[key] = line.into();
        let url = format!("{}/pulls/{}/reviews", base(&pr.repo), pr.number);
        let request = json!({
            "event": "COMMENT",
            "body": "",
            "commit_id": pr.head_oid,
            "comments": [comment],
        });
        let resp = self.rest.write(Method::Post, &url, &request)?;
        let review = self
            .rest
            .read(&url, resp)?
            .ok_or("Forgejo did not post the reply")?;
        Ok(review["html_url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map_or_else(|| thread.url.clone(), str::to_string))
    }

    fn set_resolved(
        &mut self,
        pr: &PrThreads,
        thread: &Thread,
        resolved: bool,
    ) -> Result<(), String> {
        let verb = if resolved { "resolve" } else { "unresolve" };
        let path = format!("{}/pulls/comments/{}/{verb}", base(&pr.repo), thread.id);
        let resp = self.rest.write(Method::Post, &path, &json!({}))?;
        match resp.status {
            200..=299 => Ok(()),
            404 | 405 => Err(format!(
                "{} has no API to {verb} review conversations (Gitea 1.26 added it; Forgejo has none)",
                self.rest.host()
            )),
            status => Err(self.rest.refused(status, &resp.body).message()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_versions_are_told_from_gitea_ones() {
        assert_eq!(gitea_version("1.26.0"), Some((1, 26)));
        assert_eq!(gitea_version("1.25.3"), Some((1, 25)));
        assert_eq!(gitea_version("16.0.0-dev-798-568c49e1+gitea-1.22.0"), None);
        assert_eq!(gitea_version("11.0.1+gitea-1.22.0"), None);
    }

    #[test]
    fn comments_on_one_line_are_one_thread() {
        let c = |id: u64, line: u64, at: &str| json!({ "id": id, "path": "a.rs", "position": line, "created_at": at });
        let threads = group(vec![
            c(3, 5, "2026-01-01T00:02:00Z"),
            c(1, 5, "2026-01-01T00:00:00Z"),
            c(2, 9, "2026-01-01T00:01:00Z"),
        ]);
        let ids: Vec<Vec<u64>> = threads
            .iter()
            .map(|t| t.iter().map(|c| c["id"].as_u64().unwrap()).collect())
            .collect();
        assert_eq!(ids, [vec![1, 3], vec![2]]);
    }

    #[test]
    fn a_standing_request_for_changes_wins() {
        let r = |user: &str, state: &str| json!({ "user": { "login": user }, "state": state });
        assert_eq!(
            decision(&[r("a", "APPROVED"), r("b", "REQUEST_CHANGES")]).as_deref(),
            Some("changes_requested")
        );
        assert_eq!(
            decision(&[r("a", "REQUEST_CHANGES"), r("a", "APPROVED")]).as_deref(),
            Some("approved")
        );
        assert_eq!(decision(&[r("a", "COMMENT")]), None);
    }
}
