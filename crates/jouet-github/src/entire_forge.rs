//! Repositories hosted on Entire itself (`/et/PROJECT/REPO`) behind
//! [`Host`], through the `entire` CLI (see [`crate::entire`]), which finds
//! its own login: jouet never handles an Entire token.
//!
//! A trail is the PR, and its number the PR's number. Its discussions are
//! the review threads: each has a title, messages and replies, and can be
//! resolved. Its review findings (the `code_review` discussions) are not
//! listed here: `pr threads` lists them through [`crate::entire`], as for a
//! GitHub PR, with `entire:` handles. The checks are the trail's check runs
//! (Buildkite and the like) and one "Entire Gates" check made of its
//! blocking gates, so a failing gate reads as it does on GitHub; the
//! approvals gate is the review decision instead.
//!
//! The CLI has no route to a commit's files, the merge base or the
//! branch's commits, so those come from this clone's git, and say so when
//! it lacks the commit.

use crate::entire::gates::{CHECK, Gate, Gates};
use crate::entire::{self, Entire};
use crate::forge::Forge;
use crate::host::{Feature, Host};
use crate::pr::status::{
    Checks, EntireStatus, FetchResult, Fetched, Lookup, Mergeability, PrStatus, State,
};
use crate::pr::target::{Repo, Target};
use crate::pr::threads::{self, Comment, Kind, PrThreads, Pushed, Side, Thread};
use jouet_output::time;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

/// Trails listed for `--mine`.
const MINE: &str = "100";

pub struct EntireForge {
    cli: Option<Result<Entire, String>>,
    viewer: Option<String>,
}

impl Default for EntireForge {
    fn default() -> Self {
        Self::new()
    }
}

impl EntireForge {
    pub fn new() -> Self {
        Self {
            cli: None,
            viewer: None,
        }
    }

    /// The `entire` CLI, found and logged in once.
    pub fn cli(&mut self) -> Result<&Entire, String> {
        self.cli
            .get_or_insert_with(Entire::probe)
            .as_ref()
            .map_err(|e| format!("Entire-hosted repositories are read through `entire`, and {e}"))
    }

    fn read(&mut self, args: &[&str]) -> Result<Value, String> {
        self.cli()?.read(args)
    }

    /// The trail a target names, as `entire trail show --json` prints it.
    /// `None` when there is none.
    pub fn show(&mut self, target: &Target) -> Result<Option<Value>, String> {
        let repo = target.repo();
        let sel = entire::selector(repo);
        let n = match target {
            Target::Number(_, n) => *n,
            Target::Branch(_, branch) => match self.cli()?.trail(repo, None, branch)? {
                Some(trail) => trail.number,
                None => return Ok(None),
            },
        };
        match self.read(&["trail", "show", &n.to_string(), "--json", "--repo", &sel]) {
            Ok(doc) => Ok(Some(doc)),
            Err(e) if e.starts_with("no trail") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Trail `n` as it is now, past the cache, for a wait's polls.
    pub fn fresh(&mut self, repo: &Repo, n: u64) -> Result<Option<Value>, String> {
        let sel = entire::selector(repo);
        let n = n.to_string();
        match self
            .cli()?
            .output(&["trail", "show", &n, "--json", "--repo", &sel])
        {
            Ok(out) => serde_json::from_str(&out)
                .map(Some)
                .map_err(|e| format!("entire trail show printed JSON jouet could not read: {e}")),
            Err(e) if e.starts_with("no trail") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The login `entire` acts as, such as `jdx`; empty when it says none.
    fn viewer(&mut self) -> String {
        if self.viewer.is_none() {
            let user = self
                .read(&["auth", "status", "--json"])
                .ok()
                .and_then(|s| s["user"].as_str().map(login));
            self.viewer = Some(user.unwrap_or_default());
        }
        self.viewer.clone().unwrap_or_default()
    }

    /// The trail's discussions, oldest first: the conversations, without
    /// the findings Entire keeps as `code_review` discussions.
    fn discussions(&mut self, repo: &Repo, trail: u64) -> Result<Vec<Value>, String> {
        let sel = entire::selector(repo);
        let doc = self.read(&[
            "trail",
            "comment",
            "list",
            "--trail",
            &trail.to_string(),
            "--json",
            "--repo",
            &sel,
        ])?;
        let mut list: Vec<Value> = doc["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["kind"] == "discussion")
            .cloned()
            .collect();
        list.sort_by_key(|d| d["createdAt"].as_str().unwrap_or_default().to_string());
        Ok(list)
    }

    /// The review decision, from `entire trail approvals`.
    fn review(&mut self, repo: &Repo, doc: &Value) -> Result<Option<String>, String> {
        let n = doc["number"].as_u64().unwrap_or_default().to_string();
        let sel = entire::selector(repo);
        let approvals = self.read(&["trail", "approvals", &n, "--json", "--repo", &sel])?;
        let required = gate(doc, "approvals")
            .is_some_and(|g| g["blocking"] == true && g["status"] != "passed");
        Ok(decision(&approvals["approvals"], required))
    }

    fn status(&mut self, repo: &Repo, doc: &Value) -> Result<Fetched, String> {
        let number = doc["number"].as_u64().unwrap_or_default();
        let state = match doc["status"].as_str() {
            Some("merged") => State::Merged,
            Some("closed") => State::Closed,
            Some("draft") => State::Draft,
            _ => State::Open,
        };
        let m = &doc["mergeability"];
        let (mergeable, merge_state) = match m["conflict_status"].as_str() {
            Some("conflicting") => (Mergeability::Conflicting, "dirty"),
            Some("clean") if state == State::Draft => (Mergeability::Mergeable, "draft"),
            // Clean, but a gate holds it back; the gates say which.
            Some("clean") if m["mergeable"] == false => (Mergeability::Mergeable, "blocked"),
            Some("clean") => (Mergeability::Mergeable, "clean"),
            _ => (Mergeability::Unknown, "unknown"),
        };
        let mut checks = Checks::default();
        for run in runs(doc) {
            let name = run["name"].as_str().unwrap_or_default().to_string();
            if run["status"] != "completed" {
                checks.pending += 1;
                continue;
            }
            match run["conclusion"].as_str().unwrap_or_default() {
                "success" => checks.pass += 1,
                "skipped" | "neutral" | "stale" => checks.skipped += 1,
                _ => {
                    checks.fail += 1;
                    checks.failing.push(name);
                }
            }
        }
        let gates = gates(doc);
        if let Some(g) = &gates {
            match g.state.as_str() {
                "passed" => checks.pass += 1,
                "failed" => {
                    checks.fail += 1;
                    checks.failing.push(CHECK.into());
                }
                _ => checks.pending += 1,
            }
        }
        let open = matches!(state, State::Open | State::Draft);
        let unresolved = if open {
            self.discussions(repo, number)?
                .iter()
                .filter(|d| d["resolved"] != true)
                .count()
        } else {
            0
        };
        let head_oid = m["head_sha"].as_str().unwrap_or_default().to_string();
        let mut status = PrStatus {
            repo: repo.to_string(),
            forge: Some("entire"),
            host: repo.host.clone(),
            number,
            title: text(&doc["title"]),
            url: text(&doc["url"]),
            state,
            base: text(&doc["base"]),
            head: branch(doc),
            head_sha: head_oid.chars().take(10).collect(),
            head_oid,
            pushed_at: None,
            mergeable,
            merge_state: merge_state.into(),
            checks,
            review: self.review(repo, doc)?,
            unresolved_threads: u32::try_from(unresolved).unwrap_or(u32::MAX),
            auto_merge: false,
            entire: gates.map(EntireStatus::from),
            attention: Vec::new(),
        };
        if !open {
            status.review = status.review.filter(|r| r != "review_required");
        }
        Ok(Fetched::new(status))
    }

    /// A discussion as a thread, with every message and reply in it.
    fn thread(&mut self, pr: &PrThreads, d: &Value, bots: &[String]) -> Result<Thread, String> {
        let id = text(&d["id"]);
        let sel = entire::selector(&pr.repo);
        let n = pr.number.to_string();
        let shown = self.read(&[
            "trail", "comment", "show", &id, "--trail", &n, "--json", "--repo", &sel,
        ])?;
        let url = text(&shown["discussion"]["url"]);
        let url = if url.is_empty() { pr_url(pr) } else { url };
        let mut comments = Vec::new();
        for m in shown["messages"].as_array().into_iter().flatten() {
            comments.push(comment(m, &url));
            for r in m["replies"].as_array().into_iter().flatten() {
                comments.push(comment(r, &url));
            }
        }
        comments.sort_by_key(|c| c.created);
        let title = text(&d["title"]);
        if let Some(first) = comments.first_mut()
            && !title.is_empty()
            && !first.body.starts_with(&title)
        {
            first.body = format!("{title}\n\n{}", first.body);
        }
        let author = comments
            .first()
            .map_or_else(|| text(&d["lastMessageAuthor"]), |c| c.author.clone());
        Ok(Thread {
            id: handle(&id).ok_or_else(|| {
                format!("Entire sent a discussion with an ID jouet cannot read: {id}")
            })?,
            node_id: id,
            kind: Kind::Thread,
            path: None,
            line: None,
            start_line: None,
            original_line: None,
            side: Side::Right,
            outdated: false,
            resolved: d["resolved"] == true,
            is_bot: threads::is_bot(&author, bots),
            author,
            state: None,
            url,
            comments,
            context: None,
        })
    }
}

/// `github:jdx` as Entire names a user, as the login trails show: `jdx`.
fn login(user: &str) -> String {
    user.split_once(':').map_or(user, |(_, l)| l).to_string()
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// The trail's branch, or for a merged trail the one it had.
fn branch(doc: &Value) -> String {
    Some(text(&doc["branch"]))
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| text(&doc["original_branch"]))
}

fn pr_url(pr: &PrThreads) -> String {
    format!("https://entire.com/et/{}/trails/{}", pr.repo, pr.number)
}

/// The trail's check runs, as `mergeability.checks.runs` lists them.
pub fn runs(doc: &Value) -> &[Value] {
    doc["mergeability"]["checks"]["runs"]
        .as_array()
        .map_or(&[], Vec::as_slice)
}

fn gate<'a>(doc: &'a Value, key: &str) -> Option<&'a Value> {
    doc["mergeability"]["gates"]
        .as_array()?
        .iter()
        .find(|g| g["gate_key"] == key)
}

/// The trail's blocking gates as one Entire Gates check, the way the check
/// GitHub shows reads them: failed if any fails, pending while any has not
/// been evaluated, else passed. The approvals gate is left out, since it is
/// the review decision. `None` when the trail has no other blocking gate.
pub fn gates(doc: &Value) -> Option<Gates> {
    let gates: Vec<Gate> = doc["mergeability"]["gates"]
        .as_array()?
        .iter()
        .filter(|g| g["blocking"] == true && g["gate_key"] != "approvals")
        .map(|g| Gate {
            gate: text(&g["gate_key"]),
            status: match g["status"].as_str() {
                Some(s @ ("passed" | "failed" | "skipped")) => s.to_string(),
                _ => "pending".into(),
            },
            reason: g["rationale"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map(str::to_string),
        })
        .collect();
    if gates.is_empty() {
        return None;
    }
    let state = if gates.iter().any(Gate::failed) {
        "failed"
    } else if gates.iter().any(|g| g.status == "pending") {
        "pending"
    } else {
        "passed"
    };
    Some(Gates {
        state: state.into(),
        title: None,
        gates,
        trail: doc["number"].as_u64(),
        url: doc["url"].as_str().map(str::to_string),
    })
}

/// The review decision: changes requested if anyone's latest decision asks
/// for them, else approved if one approves, else `review_required` when
/// the approvals gate blocks.
fn decision(approvals: &Value, required: bool) -> Option<String> {
    let mut list: Vec<&Value> = approvals.as_array().into_iter().flatten().collect();
    list.sort_by_key(|a| a["createdAt"].as_str().unwrap_or_default().to_string());
    let mut latest: BTreeMap<&str, &str> = BTreeMap::new();
    for a in list {
        if let (Some(author), Some(event)) = (a["author"].as_str(), a["event"].as_str()) {
            latest.insert(author, event);
        }
    }
    if latest.values().any(|e| *e == "changes_requested") {
        Some("changes_requested".into())
    } else if latest.values().any(|e| *e == "approved") {
        Some("approved".into())
    } else {
        required.then(|| "review_required".into())
    }
}

/// The number jouet names a discussion by: the first 12 characters of its
/// ULID, the 48-bit time it was made and 12 random bits, read as Crockford
/// base 32. Stable for as long as the discussion lasts.
pub(crate) fn handle(ulid: &str) -> Option<u64> {
    const DIGITS: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if ulid.len() < 12 {
        return None;
    }
    ulid[..12].chars().try_fold(0u64, |n, c| {
        let d = DIGITS.find(c.to_ascii_uppercase())?;
        Some(n << 5 | d as u64)
    })
}

fn comment(m: &Value, url: &str) -> Comment {
    let created_at = text(&m["createdAt"]);
    Comment {
        id: handle(m["id"].as_str().unwrap_or_default()).unwrap_or_default(),
        author: m["author"].as_str().unwrap_or("ghost").to_string(),
        created: time::parse_rfc3339(&created_at).unwrap_or(0),
        created_at,
        body: text(&m["body"]),
        body_truncated: false,
        suggestion: None,
        url: url.to_string(),
    }
}

/// `git ARGS` in this clone, its stdout when it succeeds.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Host for EntireForge {
    fn forge(&self) -> Forge {
        Forge::Entire
    }

    fn lacks(&mut self, feature: Feature) -> Option<String> {
        match feature {
            Feature::Search => Some(
                "--search takes GitHub's search syntax, which Entire has no equivalent of".into(),
            ),
            Feature::Resolve => None,
            Feature::Land => Some(
                "`pr land` merges GitHub PRs only so far, and the entire CLI has no merge command; merge the trail on entire.com".into(),
            ),
            Feature::Runs => Some(
                "Entire gives a trail's checks but no runs or jobs of its own; wait on the trail with `jouet wait ci` instead".into(),
            ),
            Feature::Explain => Some(
                "`ci explain` judges a failure by GitHub Actions' run history, which Entire does not give; use `jouet wait ci` for the failed checks".into(),
            ),
            Feature::RequiredChecks => Some(
                "Entire's CLI does not say which checks are required, so --required would select none; name them with --check".into(),
            ),
        }
    }

    fn fetch(&mut self, lookups: Vec<Lookup>) -> Result<FetchResult, String> {
        let mut result = FetchResult::default();
        for lookup in lookups {
            match lookup {
                Lookup::Pr(target) => match self.show(&target)? {
                    Some(doc) => result.prs.push(self.status(target.repo(), &doc)?),
                    None => result.not_found.push(target.to_string()),
                },
                Lookup::Mine(repo) => {
                    let viewer = self.viewer();
                    if viewer.is_empty() {
                        return Err(
                            "--mine needs to know who you are, and `entire auth status` names no user; run `entire login`".into(),
                        );
                    }
                    let sel = entire::selector(&repo);
                    let list = self.read(&[
                        "trail",
                        "list",
                        "--author",
                        &viewer,
                        "--status",
                        "open,draft",
                        "--json",
                        "-n",
                        MINE,
                        "--repo",
                        &sel,
                    ])?;
                    for t in list.as_array().into_iter().flatten() {
                        let Some(n) = t["number"].as_u64() else {
                            continue;
                        };
                        if let Some(doc) = self.show(&Target::Number(repo.clone(), n))? {
                            result.prs.push(self.status(&repo, &doc)?);
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
        _extra: bool,
        bots: &[String],
    ) -> Result<Option<PrThreads>, String> {
        let Some(doc) = self.show(target)? else {
            return Ok(None);
        };
        let repo = target.repo().clone();
        let number = doc["number"].as_u64().unwrap_or_default();
        let viewer = self.viewer();
        let mut pr = PrThreads {
            repo: repo.clone(),
            number,
            node_id: text(&doc["trail_id"]),
            head_ref: branch(&doc),
            head_oid: text(&doc["mergeability"]["head_sha"]),
            // The base branch's head as this clone last fetched it, for the
            // merge base.
            base_oid: git(&["rev-parse", &format!("origin/{}", text(&doc["base"]))])
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            head_repo: None,
            viewer,
            threads: Vec::new(),
            others: Vec::new(),
            unreadable: 0,
        };
        for d in self.discussions(&repo, number)? {
            let thread = self.thread(&pr, &d, bots)?;
            pr.threads.push(thread);
        }
        Ok(Some(pr))
    }

    /// Entire keeps no push log the CLI gives, so a commit's push time is
    /// its commit time in this clone, which can only be earlier.
    fn pushed_at(&mut self, pr: &PrThreads, sha: &str) -> Result<Pushed, threads::Lookup> {
        let sha = threads::sha_prefix(sha)?;
        let log = git(&["log", "--format=%H %ct", "--reverse", &pr.head_oid]).ok_or_else(|| {
            threads::Lookup::NotFound(format!(
                "this clone does not have the trail's head {}; fetch {} first",
                &pr.head_oid[..pr.head_oid.len().min(10)],
                pr.head_ref
            ))
        })?;
        let commits: Vec<(String, i64)> = log
            .lines()
            .filter_map(|l| {
                let (oid, at) = l.split_once(' ')?;
                Some((oid.to_string(), at.parse().ok()?))
            })
            .collect();
        threads::pushed_from(&commits, &[], &sha)
    }

    fn files(
        &mut self,
        _pr: &PrThreads,
        oid: &str,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        Ok(paths
            .iter()
            .filter_map(|p| Some((p.clone(), git(&["show", &format!("{oid}:{p}")])?)))
            .collect())
    }

    fn merge_base(&mut self, pr: &PrThreads) -> Result<String, String> {
        let found = (!pr.base_oid.is_empty())
            .then(|| git(&["merge-base", &pr.head_oid, &pr.base_oid]))
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        found.ok_or_else(|| {
            "Entire's CLI gives no merge base, and git found none in this clone; fetch the trail's base and head".into()
        })
    }

    fn on_branch(&mut self, pr: &PrThreads, head: &str) -> Result<bool, String> {
        Err(format!(
            "this clone does not have the trail's head, and Entire's CLI cannot say whether {} is on {}; fetch the branch first",
            &head[..head.len().min(10)],
            pr.head_ref
        ))
    }

    fn reply(&mut self, pr: &PrThreads, thread: &Thread, body: &str) -> Result<String, String> {
        let sel = entire::selector(&pr.repo);
        let n = pr.number.to_string();
        self.cli()?.output(&[
            "trail",
            "comment",
            "reply",
            &thread.node_id,
            "-m",
            body,
            "--trail",
            &n,
            "--repo",
            &sel,
        ])?;
        Ok(thread.url.clone())
    }

    fn set_resolved(
        &mut self,
        pr: &PrThreads,
        thread: &Thread,
        resolved: bool,
    ) -> Result<(), String> {
        let verb = if resolved { "resolve" } else { "unresolve" };
        let sel = entire::selector(&pr.repo);
        let n = pr.number.to_string();
        self.cli()?.output(&[
            "trail",
            "comment",
            verb,
            &thread.node_id,
            "--trail",
            &n,
            "--repo",
            &sel,
        ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn discussion_handles_are_stable_numbers() {
        let a = handle("01M67M0N092XMZ9EQMDZKWV4H9").unwrap();
        assert_eq!(handle("01m67m0n092xmz9eqmdzkwv4h9"), Some(a));
        assert_ne!(handle("01M67M0N093XMZ9EQMDZKWV4H9"), Some(a));
        assert_eq!(handle("short"), None);
        assert_eq!(handle("01M67M0N092XAAAAAAAAAAAAAA"), Some(a));
        assert_eq!(handle("01M67M0N0U2XMZ9EQMDZKWV4H9"), None);
    }

    #[test]
    fn the_latest_decision_of_each_reviewer_counts() {
        let a = |who: &str, event: &str, at: &str| json!({ "author": who, "event": event, "createdAt": at });
        let list = json!([
            a("x", "changes_requested", "2026-10-31T10:00:00Z"),
            a("x", "approved", "2026-10-31T11:00:00Z"),
        ]);
        assert_eq!(decision(&list, true).as_deref(), Some("approved"));
        let list = json!([
            a("x", "approved", "2026-10-31T10:00:00Z"),
            a("y", "changes_requested", "2026-10-31T09:00:00Z"),
        ]);
        assert_eq!(decision(&list, false).as_deref(), Some("changes_requested"));
        assert_eq!(
            decision(&json!([]), true).as_deref(),
            Some("review_required")
        );
        assert_eq!(decision(&json!([]), false), None);
    }
}
