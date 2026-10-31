//! GitLab merge requests behind [`Host`], through the REST API (v4).
//!
//! An MR's status is read from the MR itself (its `detailed_merge_status`
//! gives mergeability and what blocks it), its approvals, its discussions
//! and the commit statuses of its head, which hold the pipeline's jobs and
//! any external statuses. A review thread is a resolvable discussion: its
//! ID is its first note's, its handle for replies and resolving is the
//! discussion's own ID.

use crate::forge::Forge;
use crate::host::{Feature, Host};
use crate::pr::status::{Checks, FetchResult, Fetched, Lookup, Mergeability, PrStatus, State};
use crate::pr::target::{Repo, Target};
use crate::pr::threads::{self, Activity, Comment, Kind, PrThreads, Pushed, Side, Thread};
use crate::rest::{Rest, encode, utc};
use crate::{Failure, Method};
use jouet_output::time;
use serde_json::{Value, json};

pub struct GitLab {
    rest: Rest,
}

impl GitLab {
    pub fn new(host: &str) -> Self {
        Self {
            rest: Rest::new(Forge::GitLab, host),
        }
    }

    /// The REST client, for the CI reads `wait` makes.
    pub fn rest(&mut self) -> &mut Rest {
        &mut self.rest
    }

    /// The MR a target names: a number as is, a branch through its open MR,
    /// else its newest. `None` when there is none.
    pub fn number(&mut self, target: &Target) -> Result<Option<u64>, Failure> {
        let (repo, branch) = match target {
            Target::Number(_, n) => return Ok(Some(*n)),
            Target::Branch(repo, branch) => (repo, branch),
        };
        for state in ["&state=opened", ""] {
            let path = format!(
                "{}/merge_requests?source_branch={}{state}&order_by=created_at&sort=desc&per_page=1",
                project(repo),
                encode(branch)
            );
            if let Some(Value::Array(mrs)) = self.rest.json(&path)?
                && let Some(n) = mrs.first().and_then(|m| m["iid"].as_u64())
            {
                return Ok(Some(n));
            }
        }
        Ok(None)
    }

    fn mr(&mut self, repo: &Repo, n: u64) -> Result<Option<Value>, Failure> {
        self.rest.json(&format!(
            "{}/merge_requests/{n}?with_merge_status_recheck=true",
            project(repo)
        ))
    }

    /// An MR's status, as `pr status` reports a PR.
    fn status(&mut self, repo: &Repo, mr: &Value) -> Result<Fetched, Failure> {
        let base = project(repo);
        let n = mr["iid"].as_u64().unwrap_or_default();
        let state = match mr["state"].as_str() {
            Some("merged") => State::Merged,
            Some("closed" | "locked") => State::Closed,
            _ if mr["draft"].as_bool() == Some(true)
                || mr["work_in_progress"].as_bool() == Some(true) =>
            {
                State::Draft
            }
            _ => State::Open,
        };
        let detailed = mr["detailed_merge_status"]
            .as_str()
            .or(mr["merge_status"].as_str())
            .unwrap_or("unchecked");
        let (mergeable, merge_state) = merge_state(detailed, mr["has_conflicts"].as_bool());
        let head_oid = mr["sha"].as_str().unwrap_or_default().to_string();
        let approvals = self
            .rest
            .json(&format!("{base}/merge_requests/{n}/approvals"))
            .unwrap_or(None);
        let review = if detailed == "requested_changes" {
            Some("changes_requested".to_string())
        } else {
            approvals.as_ref().and_then(review)
        };
        let discussions = self
            .rest
            .list(&format!(
                "{base}/merge_requests/{n}/discussions?per_page=100"
            ))?
            .unwrap_or_default();
        let unresolved = discussions.iter().filter(|d| unresolved(d)).count();
        let mut checks = Checks::default();
        if !head_oid.is_empty() {
            for s in statuses(&mut self.rest, repo, &head_oid)? {
                let name = s["name"].as_str().unwrap_or_default().to_string();
                match bucket(&s) {
                    Bucket::Pass => checks.pass += 1,
                    Bucket::Fail => {
                        checks.fail += 1;
                        checks.failing.push(name);
                    }
                    Bucket::Pending => checks.pending += 1,
                    Bucket::Skipped => checks.skipped += 1,
                }
            }
        }
        let pushed_at = self
            .rest
            .list(&format!("{base}/merge_requests/{n}/versions?per_page=1"))
            .ok()
            .flatten()
            .and_then(|v| utc(&v.first()?["created_at"]));
        Ok(Fetched::new(PrStatus {
            repo: repo.to_string(),
            number: n,
            title: mr["title"].as_str().unwrap_or_default().to_string(),
            url: mr["web_url"].as_str().unwrap_or_default().to_string(),
            state,
            base: mr["target_branch"].as_str().unwrap_or_default().to_string(),
            head: mr["source_branch"].as_str().unwrap_or_default().to_string(),
            head_sha: head_oid.chars().take(10).collect(),
            head_oid,
            forge: Some("gitlab"),
            host: repo.host.clone(),
            pushed_at,
            mergeable,
            merge_state: merge_state.to_string(),
            checks,
            review,
            unresolved_threads: u32::try_from(unresolved).unwrap_or(u32::MAX),
            auto_merge: mr["merge_when_pipeline_succeeds"].as_bool() == Some(true)
                || mr["auto_merge_enabled"].as_bool() == Some(true),
            attention: Vec::new(),
        }))
    }

    /// The MR's commits, oldest first, with their commit times.
    fn commits(&mut self, pr: &PrThreads) -> Result<Vec<(String, i64)>, String> {
        let list = self
            .rest
            .list(&format!(
                "{}/merge_requests/{}/commits?per_page=100",
                project(&pr.repo),
                pr.number
            ))?
            .unwrap_or_default();
        let mut out: Vec<(String, i64)> = list
            .iter()
            .map(|c| {
                let oid = c["id"].as_str().unwrap_or_default().to_string();
                let at = c["committed_date"]
                    .as_str()
                    .and_then(time::parse_rfc3339)
                    .unwrap_or(0);
                (oid, at)
            })
            .collect();
        out.reverse();
        Ok(out)
    }
}

/// `/projects/GROUP%2FSUB%2FNAME`.
pub fn project(repo: &Repo) -> String {
    format!("/projects/{}", encode(&repo.to_string()))
}

/// Mergeability and a GitHub-style merge state from GitLab's
/// `detailed_merge_status`.
fn merge_state(detailed: &str, has_conflicts: Option<bool>) -> (Mergeability, &'static str) {
    let clean = if has_conflicts == Some(true) {
        Mergeability::Conflicting
    } else {
        Mergeability::Mergeable
    };
    match detailed {
        "mergeable" | "can_be_merged" => (Mergeability::Mergeable, "clean"),
        "conflict" | "broken_status" | "cannot_be_merged" => (Mergeability::Conflicting, "dirty"),
        "need_rebase" => (clean, "behind"),
        "draft_status" => (clean, "draft"),
        "checking"
        | "unchecked"
        | "preparing"
        | "approvals_syncing"
        | "cannot_be_merged_recheck" => (Mergeability::Unknown, "unknown"),
        _ => (clean, "blocked"),
    }
}

/// `approved` once nothing more is required and someone approved,
/// `review_required` while approvals are still needed.
fn review(approvals: &Value) -> Option<String> {
    let left = approvals["approvals_left"].as_u64().unwrap_or(0);
    let approved_by = approvals["approved_by"]
        .as_array()
        .is_some_and(|a| !a.is_empty());
    if left > 0 {
        Some("review_required".into())
    } else if approved_by {
        Some("approved".into())
    } else {
        None
    }
}

/// A resolvable discussion with a note still unresolved.
fn unresolved(discussion: &Value) -> bool {
    let notes = discussion["notes"].as_array().cloned().unwrap_or_default();
    notes
        .iter()
        .any(|n| n["resolvable"].as_bool() == Some(true) && n["resolved"].as_bool() != Some(true))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Pass,
    Fail,
    Pending,
    Skipped,
}

/// How a commit status (a job or an external status) counts. A job allowed
/// to fail passes with a warning, as GitLab shows it.
fn bucket(s: &Value) -> Bucket {
    match s["status"].as_str().unwrap_or_default() {
        "success" => Bucket::Pass,
        "failed" if s["allow_failure"].as_bool() == Some(true) => Bucket::Skipped,
        "failed" | "canceled" | "canceling" => Bucket::Fail,
        "skipped" | "manual" => Bucket::Skipped,
        _ => Bucket::Pending,
    }
}

/// The latest commit status of each name on `sha`: the jobs of its
/// pipelines and any external statuses.
pub fn statuses(rest: &mut Rest, repo: &Repo, sha: &str) -> Result<Vec<Value>, Failure> {
    Ok(rest
        .list(&format!(
            "{}/repository/commits/{sha}/statuses?per_page=100",
            project(repo)
        ))?
        .unwrap_or_default())
}

impl Host for GitLab {
    fn forge(&self) -> Forge {
        Forge::GitLab
    }

    fn lacks(&mut self, feature: Feature) -> Option<String> {
        match feature {
            Feature::Search => Some(
                "--search takes GitHub's search syntax, which GitLab has no equivalent of".into(),
            ),
            Feature::Explain => Some(
                "`ci explain` judges a failure by GitHub Actions' run history; GitLab pipelines have no equivalent jouet reads yet, so use `jouet wait ci` for the failed jobs and their logs".into(),
            ),
            Feature::Land => Some(
                "`pr land` merges GitHub PRs only so far; merge the MR with `glab mr merge` or on the web".into(),
            ),
            Feature::Resolve | Feature::Runs => None,
        }
    }

    fn fetch(&mut self, lookups: Vec<Lookup>) -> Result<FetchResult, String> {
        let mut result = FetchResult::default();
        for lookup in lookups {
            match lookup {
                Lookup::Pr(target) => {
                    let repo = target.repo().clone();
                    let mr = match self.number(&target)? {
                        Some(n) => self.mr(&repo, n)?,
                        None => None,
                    };
                    match mr {
                        Some(mr) => result.prs.push(self.status(&repo, &mr)?),
                        None => result.not_found.push(target.to_string()),
                    }
                }
                Lookup::Mine(repo) => {
                    let path = format!(
                        "{}/merge_requests?scope=created_by_me&state=opened&per_page=100",
                        project(&repo)
                    );
                    for listed in self.rest.list(&path)?.unwrap_or_default() {
                        let Some(n) = listed["iid"].as_u64() else {
                            continue;
                        };
                        if let Some(mr) = self.mr(&repo, n)? {
                            result.prs.push(self.status(&repo, &mr)?);
                        }
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
        let Some(mr) = self.mr(&repo, n)? else {
            return Ok(None);
        };
        let base = project(&repo);
        let head_oid = mr["sha"].as_str().unwrap_or_default().to_string();
        let diff_head = mr["diff_refs"]["head_sha"].as_str().unwrap_or(&head_oid);
        let head_repo = if mr["source_project_id"] != mr["target_project_id"] {
            let path = format!("/projects/{}", mr["source_project_id"]);
            self.rest
                .json(&path)
                .ok()
                .flatten()
                .and_then(|p| p["path_with_namespace"].as_str().map(str::to_string))
                .and_then(|p| Repo::on(self.rest.host(), &p))
        } else {
            None
        };
        // Reading a public MR needs no token, and then there is no viewer.
        let viewer = self
            .rest
            .json("/user")
            .ok()
            .flatten()
            .and_then(|u| u["username"].as_str().map(str::to_string))
            .unwrap_or_default();
        let web = mr["web_url"].as_str().unwrap_or_default().to_string();
        let mut pr = PrThreads {
            repo: repo.clone(),
            number: n,
            node_id: String::new(),
            head_ref: mr["source_branch"].as_str().unwrap_or_default().to_string(),
            base_oid: mr["diff_refs"]["base_sha"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            head_oid: head_oid.clone(),
            head_repo,
            viewer,
            threads: Vec::new(),
            others: Vec::new(),
            unreadable: 0,
        };
        let discussions = self
            .rest
            .list(&format!(
                "{base}/merge_requests/{n}/discussions?per_page=100"
            ))?
            .unwrap_or_default();
        for d in discussions {
            let notes: Vec<&Value> = d["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|n| n["system"].as_bool() != Some(true))
                .collect();
            let Some(first) = notes.first() else {
                continue;
            };
            let comments: Vec<Comment> = notes.iter().map(|n| note(n, &web)).collect();
            let author = comments[0].author.clone();
            let resolvable = first["resolvable"].as_bool() == Some(true);
            if !resolvable {
                if extra && d["individual_note"].as_bool() == Some(true) {
                    pr.others.push(thread(Kind::Comment, &d, comments, bots));
                }
                continue;
            }
            let mut t = thread(Kind::Thread, &d, comments, bots);
            t.resolved = !unresolved(&d);
            let position = &first["position"];
            if position.is_object() {
                let (side, line, path) = match position["new_line"].as_u64() {
                    Some(line) => (Side::Right, Some(line), position["new_path"].as_str()),
                    None => (
                        Side::Left,
                        position["old_line"].as_u64(),
                        position["old_path"].as_str(),
                    ),
                };
                let line = line.and_then(|l| u32::try_from(l).ok());
                let range = &position["line_range"]["start"];
                let start = match side {
                    Side::Right => range["new_line"].as_u64(),
                    Side::Left => range["old_line"].as_u64(),
                }
                .and_then(|l| u32::try_from(l).ok())
                .filter(|s| Some(*s) != line);
                t.outdated = position["head_sha"]
                    .as_str()
                    .is_some_and(|h| h != diff_head);
                t.path = path.map(str::to_string);
                t.side = side;
                t.original_line = line;
                t.line = line.filter(|_| !t.outdated);
                t.start_line = start.filter(|_| !t.outdated);
            }
            t.is_bot = threads::is_bot(&author, bots);
            pr.threads.push(t);
        }
        pr.others
            .sort_by_key(|t| t.comments.first().map_or(0, |c| c.created));
        Ok(Some(pr))
    }

    fn pushed_at(&mut self, pr: &PrThreads, sha: &str) -> Result<Pushed, threads::Lookup> {
        let sha = threads::sha_prefix(sha)?;
        let commits = self.commits(pr).map_err(threads::Lookup::Remote)?;
        // Each version of an MR is one push of its branch.
        let versions = self
            .rest
            .list(&format!(
                "{}/merge_requests/{}/versions?per_page=100",
                project(&pr.repo),
                pr.number
            ))
            .map_err(|e| threads::Lookup::Remote(e.message()))?
            .unwrap_or_default();
        let activity: Vec<Activity> = versions
            .iter()
            .filter_map(|v| {
                Some(Activity {
                    after: v["head_commit_sha"].as_str()?.to_string(),
                    timestamp: utc(&v["created_at"])?,
                })
            })
            .collect();
        threads::pushed_from(&commits, &activity, &sha)
    }

    fn files(
        &mut self,
        pr: &PrThreads,
        oid: &str,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for path in paths {
            let url = format!(
                "{}/repository/files/{}/raw?ref={oid}",
                project(&pr.repo),
                encode(path)
            );
            let resp = self.rest.get(&url, None)?;
            if resp.status == 200 {
                out.push((path.clone(), resp.body));
            }
        }
        Ok(out)
    }

    /// GitLab's diff is taken against the `base_sha` of the MR's diff refs,
    /// which is the merge base.
    fn merge_base(&mut self, pr: &PrThreads) -> Result<String, String> {
        if pr.base_oid.is_empty() {
            return Err("GitLab sent the MR without its diff's base".into());
        }
        Ok(pr.base_oid.clone())
    }

    fn on_branch(&mut self, pr: &PrThreads, head: &str) -> Result<bool, String> {
        Ok(self.commits(pr)?.iter().any(|(oid, _)| oid == head))
    }

    fn reply(&mut self, pr: &PrThreads, thread: &Thread, body: &str) -> Result<String, String> {
        let path = format!(
            "{}/merge_requests/{}/discussions/{}/notes",
            project(&pr.repo),
            pr.number,
            thread.node_id
        );
        let resp = self
            .rest
            .write(Method::Post, &path, &json!({ "body": body }))?;
        let note = self
            .rest
            .read(&path, resp)?
            .ok_or("GitLab did not post the reply")?;
        let id = note["id"].as_u64().ok_or("GitLab did not post the reply")?;
        let web = thread.url.split('#').next().unwrap_or_default();
        Ok(format!("{web}#note_{id}"))
    }

    fn set_resolved(
        &mut self,
        pr: &PrThreads,
        thread: &Thread,
        resolved: bool,
    ) -> Result<(), String> {
        let path = format!(
            "{}/merge_requests/{}/discussions/{}?resolved={resolved}",
            project(&pr.repo),
            pr.number,
            thread.node_id
        );
        let resp = self.rest.write(Method::Put, &path, &json!({}))?;
        let discussion = self
            .rest
            .read(&path, resp)?
            .ok_or("GitLab no longer has the discussion")?;
        if unresolved(&discussion) == resolved {
            let verb = if resolved { "resolve" } else { "unresolve" };
            return Err(format!("GitLab did not {verb} the thread"));
        }
        Ok(())
    }
}

/// A thread of `kind` from a discussion's notes.
fn thread(kind: Kind, d: &Value, comments: Vec<Comment>, bots: &[String]) -> Thread {
    let first = &comments[0];
    Thread {
        id: first.id,
        node_id: d["id"].as_str().unwrap_or_default().to_string(),
        kind,
        path: None,
        line: None,
        start_line: None,
        original_line: None,
        side: Side::Right,
        outdated: false,
        resolved: false,
        is_bot: threads::is_bot(&first.author, bots),
        author: first.author.clone(),
        state: None,
        url: first.url.clone(),
        comments,
        context: None,
    }
}

fn note(n: &Value, web: &str) -> Comment {
    let id = n["id"].as_u64().unwrap_or_default();
    let created_at = utc(&n["created_at"]).unwrap_or_default();
    Comment {
        id,
        author: n["author"]["username"]
            .as_str()
            .unwrap_or("ghost")
            .to_string(),
        created: time::parse_utc(&created_at).unwrap_or(0),
        created_at,
        body: n["body"].as_str().unwrap_or_default().to_string(),
        body_truncated: false,
        suggestion: None,
        url: format!("{web}#note_{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_states_map_to_githubs() {
        assert_eq!(
            merge_state("mergeable", Some(false)),
            (Mergeability::Mergeable, "clean")
        );
        assert_eq!(
            merge_state("conflict", Some(true)),
            (Mergeability::Conflicting, "dirty")
        );
        assert_eq!(
            merge_state("need_rebase", Some(false)),
            (Mergeability::Mergeable, "behind")
        );
        assert_eq!(
            merge_state("checking", None),
            (Mergeability::Unknown, "unknown")
        );
        assert_eq!(
            merge_state("not_approved", Some(false)),
            (Mergeability::Mergeable, "blocked")
        );
    }

    #[test]
    fn a_job_allowed_to_fail_passes_with_a_warning() {
        assert_eq!(
            bucket(&json!({"status": "failed", "allow_failure": true})),
            Bucket::Skipped
        );
        assert_eq!(bucket(&json!({"status": "failed"})), Bucket::Fail);
        assert_eq!(bucket(&json!({"status": "running"})), Bucket::Pending);
        assert_eq!(bucket(&json!({"status": "manual"})), Bucket::Skipped);
    }

    #[test]
    fn group_paths_are_one_encoded_project_id() {
        let repo = Repo::parse("gitlab.com/group/sub/project").unwrap();
        assert_eq!(project(&repo), "/projects/group%2Fsub%2Fproject");
    }
}
