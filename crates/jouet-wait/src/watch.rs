//! The shared, resumable state of a PR being watched by `wait ci`.
//!
//! Each PR has one state file, `<owner>-<repo>-<pr>.json` under
//! [`paths::watch_dir`], guarded by a lock file beside it. A wait takes the
//! lock, and polls GitHub only when the state is older than its own
//! interval; otherwise it reads what another wait (in this session or
//! another) already fetched. So any number of waits on one PR cost about
//! one wait's requests, and no daemon is needed.
//!
//! The state also keeps the ETags of every response it was built from, so
//! even the first poll by a new wait can be answered with free 304s, and a
//! log of events for the current head commit, which a restarted wait
//! replays.
//!
//! [`paths::watch_dir`]: jouet_output::paths::watch_dir

use super::checks::Check;
use jouet_github::entire::gates;
use jouet_github::pr::target::Repo;
use jouet_github::{Client, Failure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Events kept in a state file; older ones are dropped first.
const MAX_EVENTS: usize = 1000;
/// Pages of check runs or files read at most (100 each).
const MAX_PAGES: u32 = 30;

/// The PR fields a wait needs, from `GET /repos/{o}/{r}/pulls/{n}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pr {
    pub(crate) number: u64,
    /// `open`, `closed` or `merged`.
    pub(crate) state: String,
    pub(crate) head_sha: String,
    pub(crate) base_ref: String,
    /// `None` while GitHub is still computing it.
    pub(crate) mergeable: Option<bool>,
    pub(crate) mergeable_state: String,
}

impl Pr {
    fn from_rest(v: &Value) -> Option<Self> {
        let state = if v["merged"].as_bool() == Some(true) || v["merged_at"].is_string() {
            "merged"
        } else {
            v["state"].as_str()?
        };
        Some(Self {
            number: v["number"].as_u64()?,
            state: state.to_string(),
            head_sha: v["head"]["sha"].as_str()?.to_string(),
            base_ref: v["base"]["ref"].as_str()?.to_string(),
            mergeable: v["mergeable"].as_bool(),
            mergeable_state: v["mergeable_state"].as_str().unwrap_or("unknown").into(),
        })
    }

    /// GitHub has finished computing mergeability and found a conflict.
    pub(crate) fn conflicted(&self) -> bool {
        self.mergeable == Some(false) && self.mergeable_state == "dirty"
    }
}

/// A response body kept for its ETag, reduced to the fields jouet reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Cached {
    pub(crate) etag: String,
    pub(crate) data: Value,
}

/// A recorded event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Logged {
    pub(crate) seq: u64,
    /// When it was seen, in Unix seconds.
    pub(crate) at: i64,
    pub(crate) name: String,
    pub(crate) fields: Value,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Snapshot {
    /// When GitHub was last polled, in Unix milliseconds.
    pub(crate) polled_at_ms: u64,
    pub(crate) pr: Option<Pr>,
    /// The commit whose checks are listed: the PR head, or a pinned SHA.
    pub(crate) sha: Option<String>,
    pub(crate) checks: Vec<Check>,
    /// Check names whose required-ness has been asked, and those required.
    pub(crate) required_known: BTreeSet<String>,
    pub(crate) required: BTreeSet<String>,
    pub(crate) base_sha: Option<String>,
    pub(crate) cache: BTreeMap<String, Cached>,
    pub(crate) events: Vec<Logged>,
    pub(crate) seq: u64,
    /// Set while the primary rate limit is used up, so other waits do not
    /// spend requests finding that out.
    pub(crate) rate_limited_until: Option<i64>,
}

impl Snapshot {
    fn log(&mut self, name: &str, fields: Value, text: String) {
        self.seq += 1;
        self.events.push(Logged {
            seq: self.seq,
            at: jouet_output::time::now(),
            name: name.into(),
            fields,
            text,
        });
        if self.events.len() > MAX_EVENTS {
            self.events.drain(..self.events.len() - MAX_EVENTS);
        }
    }
}

/// Why a poll stopped.
#[derive(Debug)]
pub(crate) enum PollError {
    NotFound(String),
    Remote(Failure),
}

impl From<Failure> for PollError {
    fn from(f: Failure) -> Self {
        Self::Remote(f)
    }
}

/// How far apart two reports of a conflict must be before a wait believes
/// it. For a second or so after a push, GitHub's API still answers for the
/// old head, mergeability included, so a conflict that a rebase just fixed
/// can look unfixed; asking again later tells the two apart.
pub(crate) const CONFLICT_CONFIRM: Duration = Duration::from_secs(10);

/// The message while a conflict waits to be confirmed.
pub(crate) const CONFIRMING: &str =
    "GitHub reports a conflict; asking again to rule out a stale answer";

/// Confirms a conflict by seeing it again `window` later
/// ([`CONFLICT_CONFIRM`] unless a test asks for less).
#[derive(Debug)]
pub(crate) struct ConflictCheck {
    first_seen_ms: Option<u64>,
    window_ms: u64,
}

impl ConflictCheck {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            first_seen_ms: None,
            window_ms: u64::try_from(window.as_millis()).unwrap_or(u64::MAX),
        }
    }

    /// Records whether GitHub reported a conflict when it was asked at
    /// `asked_ms` (Unix milliseconds), and says whether the conflict is now
    /// confirmed.
    pub(crate) fn confirmed(&mut self, conflicted: bool, asked_ms: u64) -> bool {
        if !conflicted {
            self.first_seen_ms = None;
            return false;
        }
        let first = *self.first_seen_ms.get_or_insert(asked_ms);
        asked_ms.saturating_sub(first) >= self.window_ms
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The state file for a PR, or for one pinned commit of it.
pub(crate) fn state_path(repo: &Repo, number: u64, pinned: Option<&str>) -> PathBuf {
    let name = match pinned {
        Some(sha) => format!("{}-{}-{number}-{sha}.json", repo.owner, repo.name),
        None => format!("{}-{}-{number}.json", repo.owner, repo.name),
    };
    jouet_output::paths::watch_dir().join(name)
}

/// Runs `f` on the state at `path` while holding its lock, then saves the
/// state, whatever `f` returned.
pub(crate) fn with_state<T>(path: &Path, f: impl FnOnce(&mut Snapshot) -> T) -> Result<T, String> {
    let dir = path.parent().ok_or("the watch state has no directory")?;
    fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let lock_path = path.with_extension("lock");
    let lock =
        File::create(&lock_path).map_err(|e| format!("opening {}: {e}", lock_path.display()))?;
    lock.lock()
        .map_err(|e| format!("locking {}: {e}", lock_path.display()))?;
    // A missing or unreadable state (such as one from an older jouet)
    // starts over; it is only a cache of what GitHub says.
    let mut snap: Snapshot = fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let out = f(&mut snap);
    let text = serde_json::to_vec(&snap).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("writing the watch state: {e}"))?;
    tmp.write_all(&text)
        .map_err(|e| format!("writing the watch state: {e}"))?;
    tmp.persist(path)
        .map_err(|e| format!("saving {}: {e}", path.display()))?;
    drop(lock);
    Ok(out)
}

/// A GET through the ETag cache: a 304 answers from the cache, a 200 is
/// reduced by `keep` and cached. `None` is a 404.
pub(crate) fn cached_get(
    client: &mut Client,
    cache: &mut BTreeMap<String, Cached>,
    path: &str,
    keep: fn(Value) -> Value,
) -> Result<Option<Value>, Failure> {
    let etag = cache.get(path).map(|c| c.etag.clone());
    let resp = client.get(path, etag.as_deref())?;
    match resp.status {
        304 => match cache.get(path) {
            Some(c) => Ok(Some(c.data.clone())),
            None => Err(Failure::Other(format!(
                "GitHub answered 304 for {path} with nothing cached"
            ))),
        },
        200 => {
            let body: Value = serde_json::from_str(&resp.body).map_err(|e| {
                Failure::Other(format!(
                    "GitHub sent {path} in a form jouet could not read: {e}"
                ))
            })?;
            let data = keep(body);
            match resp.etag {
                Some(etag) => {
                    cache.insert(
                        path.to_string(),
                        Cached {
                            etag,
                            data: data.clone(),
                        },
                    );
                }
                None => {
                    cache.remove(path);
                }
            }
            Ok(Some(data))
        }
        404 => Ok(None),
        status => Err(Failure::Other(format!(
            "GitHub returned {status} for {path}: {}",
            error_message(&resp.body)
        ))),
    }
}

fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.trim().chars().take(200).collect())
}

/// Escapes a branch name for a URL path; slashes stay, as GitHub expects.
fn path_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn keep_pr(v: Value) -> Value {
    json!({
        "number": v["number"], "state": v["state"], "merged": v["merged"],
        "merged_at": v["merged_at"], "head": { "sha": v["head"]["sha"] },
        "base": { "ref": v["base"]["ref"] }, "mergeable": v["mergeable"],
        "mergeable_state": v["mergeable_state"],
    })
}

fn keep_check_runs(v: Value) -> Value {
    let runs: Vec<Value> = v["check_runs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let mut kept = json!({
                "id": r["id"], "name": r["name"], "status": r["status"],
                "conclusion": r["conclusion"], "html_url": r["html_url"],
                "details_url": r["details_url"], "app": { "slug": r["app"]["slug"] },
            });
            // The Entire Gates check says in its summary why it failed.
            if r["name"] == gates::CHECK {
                kept["output"] =
                    json!({ "title": r["output"]["title"], "summary": r["output"]["summary"] });
            }
            kept
        })
        .collect();
    json!({ "total_count": v["total_count"], "check_runs": runs })
}

fn keep_status(v: Value) -> Value {
    let statuses: Vec<Value> = v["statuses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| json!({ "context": s["context"], "state": s["state"], "target_url": s["target_url"] }))
        .collect();
    json!({ "total_count": v["total_count"], "statuses": statuses })
}

fn keep_branch(v: Value) -> Value {
    json!({ "commit": { "sha": v["commit"]["sha"] } })
}

fn keep_files(v: Value) -> Value {
    let names = |list: &Value| -> Vec<Value> {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|f| f["filename"].clone())
            .collect()
    };
    if v.is_array() {
        Value::Array(names(&v))
    } else {
        json!({ "files": names(&v["files"]) })
    }
}

fn keep_all(v: Value) -> Value {
    v
}

/// Brings the state up to date with GitHub: the PR, the checks on its head
/// (or the pinned commit), which of them are required, and whether new base
/// commits touch the PR's files. Changes are logged as events.
pub(crate) fn poll(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    pinned: Option<&str>,
    snap: &mut Snapshot,
) -> Result<(), PollError> {
    let base = format!("/repos/{}/{}", repo.owner, repo.name);
    let pr_path = format!("{base}/pulls/{number}");
    let pr = cached_get(client, &mut snap.cache, &pr_path, keep_pr)?
        .ok_or_else(|| PollError::NotFound(format!("{repo}#{number} does not exist")))?;
    let pr = Pr::from_rest(&pr).ok_or_else(|| {
        PollError::Remote(Failure::Other(format!(
            "GitHub sent {repo}#{number} without its head or base"
        )))
    })?;
    let sha = pinned.map_or_else(|| pr.head_sha.clone(), str::to_string);
    if let Some(old) = snap.sha.as_deref()
        && old != sha
    {
        let (from, to) = (old.to_string(), sha.clone());
        snap.events.clear();
        snap.checks.clear();
        snap.required_known.clear();
        snap.required.clear();
        let text = format!("head moved from {} to {}", short(&from), short(&to));
        snap.log("head_changed", json!({ "from": from, "to": to }), text);
        // The old commit's responses will not be asked for again.
        snap.cache.retain(|k, _| !k.contains(&from));
    }
    snap.sha = Some(sha.clone());
    let checks = fetch_checks(client, &base, &sha, &mut snap.cache)?;
    update_required(client, repo, number, &sha, &checks, snap);
    let checks: Vec<Check> = checks
        .into_iter()
        .map(|mut c| {
            c.required = snap.required.contains(&c.name);
            c
        })
        .collect();
    log_check_changes(snap, &checks);
    snap.checks = checks;
    check_base(client, &base, &pr, snap)?;
    snap.pr = Some(pr);
    Ok(())
}

pub(crate) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Every check run and commit status on `sha`.
fn fetch_checks(
    client: &mut Client,
    base: &str,
    sha: &str,
    cache: &mut BTreeMap<String, Cached>,
) -> Result<Vec<Check>, Failure> {
    let mut checks = Vec::new();
    for page in 1..=MAX_PAGES {
        // `filter=latest` is GitHub's default. Naming it keys this list apart
        // from caches written before the Entire Gates summary was kept.
        let path =
            format!("{base}/commits/{sha}/check-runs?filter=latest&per_page=100&page={page}");
        let Some(v) = cached_get(client, cache, &path, keep_check_runs)? else {
            break;
        };
        let runs = v["check_runs"].as_array().cloned().unwrap_or_default();
        checks.extend(runs.iter().filter_map(Check::from_check_run));
        let total = v["total_count"].as_u64().unwrap_or(0);
        if runs.len() < 100 || checks.len() as u64 >= total {
            break;
        }
    }
    let statuses = 'statuses: {
        let mut statuses = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!("{base}/commits/{sha}/status?per_page=100&page={page}");
            let Some(v) = cached_get(client, cache, &path, keep_status)? else {
                break 'statuses statuses;
            };
            let list = v["statuses"].as_array().cloned().unwrap_or_default();
            statuses.extend(list.iter().filter_map(Check::from_status));
            if list.len() < 100 {
                break;
            }
        }
        statuses
    };
    checks.extend(statuses);
    Ok(checks)
}

/// Asks which checks on `sha` are required, for names not asked about
/// yet. Only GraphQL says this for checks on a PR (the branch protection API
/// needs admin rights), so it is asked again only when new names appear.
/// `sha` is the commit being waited on, which with `--sha` need not be the
/// PR head. A name counts as known only once GraphQL has answered for it, so
/// a check that REST lists before GraphQL does, or a failed query, is asked
/// about again at the next poll.
fn update_required(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    sha: &str,
    checks: &[Check],
    snap: &mut Snapshot,
) {
    if checks.iter().all(|c| snap.required_known.contains(&c.name)) {
        return;
    }
    const QUERY: &str = "query($o: String!, $r: String!, $n: Int!, $s: String!, $a: String) {
  repository(owner: $o, name: $r) { object(expression: $s) { ... on Commit {
    statusCheckRollup { contexts(first: 100, after: $a) {
      pageInfo { hasNextPage endCursor }
      nodes {
        ... on CheckRun { name isRequired(pullRequestNumber: $n) }
        ... on StatusContext { context isRequired(pullRequestNumber: $n) }
      }
    } }
  } } }
}";
    let mut after: Option<String> = None;
    let mut required = BTreeSet::new();
    let mut answered = BTreeSet::new();
    for _ in 0..MAX_PAGES {
        let vars = json!({ "o": repo.owner, "r": repo.name, "n": number, "s": sha, "a": after });
        let Ok(data) = client.graphql(QUERY, &vars) else {
            return;
        };
        let contexts = &data["repository"]["object"]["statusCheckRollup"]["contexts"];
        for node in contexts["nodes"].as_array().into_iter().flatten() {
            let name = node["name"].as_str().or(node["context"].as_str());
            let (Some(name), Some(is_required)) = (name, node["isRequired"].as_bool()) else {
                continue;
            };
            answered.insert(name.to_string());
            if is_required {
                required.insert(name.to_string());
            }
        }
        let page = &contexts["pageInfo"];
        match (page["hasNextPage"].as_bool(), page["endCursor"].as_str()) {
            (Some(true), Some(cursor)) => after = Some(cursor.to_string()),
            _ => break,
        }
    }
    snap.required.extend(required);
    snap.required_known.extend(answered);
}

/// Logs `check_registered` for checks not seen before and
/// `check_completed` for checks that have finished since the last poll.
fn log_check_changes(snap: &mut Snapshot, checks: &[Check]) {
    let key = |c: &Check| (c.name.clone(), c.url.clone());
    let old: BTreeMap<_, _> = snap.checks.iter().map(|c| (key(c), c.state)).collect();
    for c in checks {
        let before = old.get(&key(c));
        if before.is_none() {
            snap.log(
                "check_registered",
                json!({ "name": c.name, "state": c.state.as_str() }),
                String::new(),
            );
        }
        if c.state.finished() && before.is_none_or(|s| !s.finished()) {
            // Failures get a line of their own; passes show in the counts.
            let text = if c.state.failed() {
                format!("{} {}", c.name, c.state.as_str())
            } else {
                String::new()
            };
            snap.log(
                "check_completed",
                json!({ "name": c.name, "state": c.state.as_str(), "conclusion": c.conclusion, "url": c.url }),
                text,
            );
        }
    }
}

/// Logs `base_touched` when commits pushed to the base branch since the last
/// poll change files this PR changes.
fn check_base(
    client: &mut Client,
    base: &str,
    pr: &Pr,
    snap: &mut Snapshot,
) -> Result<(), Failure> {
    let path = format!("{base}/branches/{}", path_escape(&pr.base_ref));
    let Some(branch) = cached_get(client, &mut snap.cache, &path, keep_branch)? else {
        return Ok(());
    };
    let Some(new) = branch["commit"]["sha"].as_str().map(str::to_string) else {
        return Ok(());
    };
    let Some(old) = snap.base_sha.replace(new.clone()) else {
        return Ok(());
    };
    if old == new {
        return Ok(());
    }
    let compare = format!("{base}/compare/{old}...{new}");
    let Some(changed) = cached_get(client, &mut snap.cache, &compare, keep_files)? else {
        return Ok(());
    };
    snap.cache.remove(&compare);
    let changed: BTreeSet<&str> = changed["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut ours = BTreeSet::new();
    for page in 1..=MAX_PAGES {
        let files = format!("{base}/pulls/{}/files?per_page=100&page={page}", pr.number);
        let Some(list) = cached_get(client, &mut snap.cache, &files, keep_files)? else {
            break;
        };
        let names = list.as_array().cloned().unwrap_or_default();
        ours.extend(names.iter().filter_map(|n| n.as_str().map(str::to_string)));
        if names.len() < 100 {
            break;
        }
    }
    let touched: Vec<&String> = ours
        .iter()
        .filter(|f| changed.contains(f.as_str()))
        .collect();
    if !touched.is_empty() {
        let text = format!(
            "{} moved to {} and touches {}",
            pr.base_ref,
            short(&new),
            touched
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        snap.log(
            "base_touched",
            json!({ "base": pr.base_ref, "base_sha": new, "files": touched }),
            text,
        );
    }
    Ok(())
}

/// The run and job documents `wait run` and log fetching read, kept whole.
pub(crate) fn get_json(
    client: &mut Client,
    cache: &mut BTreeMap<String, Cached>,
    path: &str,
) -> Result<Option<Value>, Failure> {
    cached_get(client, cache, path, keep_all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conflict_counts_once_seen_again_ten_seconds_later() {
        let mut check = ConflictCheck::new(CONFLICT_CONFIRM);
        assert!(!check.confirmed(true, 1_000));
        assert!(!check.confirmed(true, 6_000));
        assert!(check.confirmed(true, 11_000));
        // A clean answer in between starts over.
        assert!(!check.confirmed(false, 12_000));
        assert!(!check.confirmed(true, 13_000));
        assert!(check.confirmed(true, 23_000));
    }

    #[test]
    fn branch_names_keep_their_slashes() {
        assert_eq!(path_escape("feat/a b#c"), "feat/a%20b%23c");
    }

    #[test]
    fn check_changes_are_logged_once() {
        let mut snap = Snapshot::default();
        let run = |status: &str, conclusion: Option<&str>| {
            Check::from_check_run(&json!({
                "id": 1, "name": "lint", "status": status, "conclusion": conclusion,
                "html_url": "https://x/actions/runs/2/job/1", "app": { "slug": "github-actions" }
            }))
            .unwrap()
        };
        let pending = vec![run("in_progress", None)];
        log_check_changes(&mut snap, &pending);
        snap.checks = pending.clone();
        log_check_changes(&mut snap, &pending);
        let done = vec![run("completed", Some("failure"))];
        log_check_changes(&mut snap, &done);
        let names: Vec<&str> = snap.events.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["check_registered", "check_completed"]);
        assert_eq!(snap.events[1].text, "lint fail");
    }
}
