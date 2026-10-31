//! Fetching the status of many PRs in batched GraphQL queries, and deriving
//! the reasons each one needs attention.
//!
//! A query holds up to 50 lookups, each under its own alias. Check counts
//! come from GitHub's per-state totals; listing the checks themselves is what
//! makes a status query slow (50 PRs with their checks can pass GitHub's 10s
//! timeout), so the names of failing checks are fetched in follow-up queries
//! only for PRs that have failures. Review threads come 100 to a page and the
//! rare PR with more is paged the same way, so counts and names are never cut
//! short.

use super::target::{Repo, Target};
use crate::Client;
use crate::entire::{Brief, CHECK, Gate, Gates};
use jouet_output::time;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Lookups per query.
const BATCH: usize = 50;
/// Pages of check contexts per query: each lists up to 100 checks.
const CONTEXTS_BATCH: usize = 20;
/// How long after a push a PR may have no checks before that is reported.
const CHECKS_GRACE_SECS: i64 = 5 * 60;

const PR_FRAGMENT: &str = "fragment pr on PullRequest {
  id number title url state isDraft
  baseRefName headRefName headRefOid
  repository { nameWithOwner }
  mergeable mergeStateStatus reviewDecision
  autoMergeRequest { enabledAt }
  reviewThreads(first: 100) { ...threads }
  timelineItems(last: 1, itemTypes: [HEAD_REF_FORCE_PUSHED_EVENT]) {
    nodes { ... on HeadRefForcePushedEvent { createdAt } }
  }
  commits(last: 1) {
    nodes { commit { committedDate statusCheckRollup { id contexts {
      checkRunCountsByState { state count }
      statusContextCountsByState { state count }
    } }
    checkSuites(last: 1, filterBy: {checkName: \"Entire Gates\"}) { nodes {
      checkRuns(last: 1, filterBy: {checkName: \"Entire Gates\"}) {
        nodes { status conclusion detailsUrl title summary }
      }
    } } } }
  }
}
";
const THREADS_FRAGMENT: &str = "fragment threads on PullRequestReviewThreadConnection {
  pageInfo { hasNextPage endCursor }
  nodes { isResolved }
}
";

/// One aliased entry in a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Pr(Target),
    /// A page of a GitHub search; `after` is the cursor of the previous page
    /// and `listed` counts the PRs earlier pages returned.
    Search {
        query: String,
        after: Option<String>,
        listed: u64,
    },
    /// The next page of a PR's review threads, by the PR's node ID.
    Threads {
        id: String,
        after: String,
    },
    /// A page of a commit's check contexts, by the rollup's node ID.
    Contexts {
        id: String,
        after: Option<String>,
    },
}

/// Builds the query and variables for a batch of lookups.
pub(crate) fn query(lookups: &[Lookup]) -> (String, Value) {
    let mut params = Vec::new();
    let mut body = String::new();
    let mut vars = Map::new();
    let (mut pr, mut threads) = (false, false);
    for (i, lookup) in lookups.iter().enumerate() {
        match lookup {
            Lookup::Pr(Target::Number(repo, n)) => {
                pr = true;
                params.extend([
                    format!("$o{i}: String!"),
                    format!("$r{i}: String!"),
                    format!("$n{i}: Int!"),
                ]);
                vars.insert(format!("o{i}"), json!(repo.owner));
                vars.insert(format!("r{i}"), json!(repo.name));
                vars.insert(format!("n{i}"), json!(n));
                let _ = writeln!(
                    body,
                    "  l{i}: repository(owner: $o{i}, name: $r{i}) {{ pullRequest(number: $n{i}) {{ ...pr }} }}"
                );
            }
            Lookup::Pr(Target::Branch(repo, branch)) => {
                pr = true;
                params.extend([
                    format!("$o{i}: String!"),
                    format!("$r{i}: String!"),
                    format!("$b{i}: String!"),
                ]);
                vars.insert(format!("o{i}"), json!(repo.owner));
                vars.insert(format!("r{i}"), json!(repo.name));
                vars.insert(format!("b{i}"), json!(branch));
                // The open PR for the branch if there is one, else the newest.
                let newest = "first: 1, orderBy: {field: CREATED_AT, direction: DESC}";
                let _ = writeln!(
                    body,
                    "  l{i}: repository(owner: $o{i}, name: $r{i}) {{ \
                     open: pullRequests(headRefName: $b{i}, states: OPEN, {newest}) {{ nodes {{ ...pr }} }} \
                     newest: pullRequests(headRefName: $b{i}, {newest}) {{ nodes {{ ...pr }} }} }}"
                );
            }
            Lookup::Search { query, after, .. } => {
                pr = true;
                params.extend([format!("$q{i}: String!"), format!("$a{i}: String")]);
                vars.insert(format!("q{i}"), json!(query));
                vars.insert(format!("a{i}"), json!(after));
                let _ = writeln!(
                    body,
                    "  l{i}: search(query: $q{i}, type: ISSUE, first: {BATCH}, after: $a{i}) {{ \
                     issueCount pageInfo {{ hasNextPage endCursor }} nodes {{ ... on PullRequest {{ ...pr }} }} }}"
                );
            }
            Lookup::Threads { id, after } => {
                threads = true;
                params.extend([format!("$id{i}: ID!"), format!("$c{i}: String!")]);
                vars.insert(format!("id{i}"), json!(id));
                vars.insert(format!("c{i}"), json!(after));
                let _ = writeln!(
                    body,
                    "  l{i}: node(id: $id{i}) {{ ... on PullRequest {{ \
                     reviewThreads(first: 100, after: $c{i}) {{ ...threads }} }} }}"
                );
            }
            Lookup::Contexts { id, after } => {
                params.extend([format!("$id{i}: ID!"), format!("$c{i}: String")]);
                vars.insert(format!("id{i}"), json!(id));
                vars.insert(format!("c{i}"), json!(after));
                let _ = writeln!(
                    body,
                    "  l{i}: node(id: $id{i}) {{ ... on StatusCheckRollup {{ \
                     contexts(first: 100, after: $c{i}) {{ pageInfo {{ hasNextPage endCursor }} \
                     nodes {{ ... on CheckRun {{ name conclusion }} ... on StatusContext {{ context state }} }} }} }} }}"
                );
            }
        }
    }
    // GraphQL rejects a document that defines a fragment it does not use.
    let mut query = format!("query({}) {{\n{body}}}\n", params.join(", "));
    if pr {
        query.push_str(PR_FRAGMENT);
    }
    if pr || threads {
        query.push_str(THREADS_FRAGMENT);
    }
    (query, Value::Object(vars))
}

// The shapes GitHub returns, as far as jouet reads them.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    id: String,
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    repository: RawRepo,
    mergeable: String,
    merge_state_status: String,
    review_decision: Option<String>,
    auto_merge_request: Option<Value>,
    review_threads: RawThreads,
    timeline_items: Nodes<RawForcePush>,
    commits: Nodes<RawCommitNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepo {
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThreads {
    page_info: PageInfo,
    nodes: Vec<RawThread>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    is_resolved: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawForcePush {
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCommitNode {
    commit: RawCommit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCommit {
    committed_date: String,
    status_check_rollup: Option<RawRollup>,
    check_suites: Option<Nodes<RawSuite>>,
}

/// A check suite holding the Entire Gates check.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSuite {
    check_runs: Nodes<RawGatesRun>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGatesRun {
    status: String,
    conclusion: Option<String>,
    details_url: Option<String>,
    title: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawRollup {
    id: String,
    contexts: RawCounts,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCounts {
    #[serde(default)]
    check_run_counts_by_state: Vec<RawCount>,
    #[serde(default)]
    status_context_counts_by_state: Vec<RawCount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawContexts {
    page_info: PageInfo,
    nodes: Vec<RawContext>,
}

#[derive(Debug, Deserialize)]
struct RawCount {
    state: String,
    count: u32,
}

/// A check run (`name`, `conclusion`) or a commit status
/// (`context`, `state`).
#[derive(Debug, Deserialize)]
struct RawContext {
    name: Option<String>,
    conclusion: Option<String>,
    context: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSearch {
    issue_count: u64,
    page_info: PageInfo,
    nodes: Vec<Value>,
}

/// How a check or commit status state counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Pass,
    Fail,
    Pending,
    Skipped,
}

/// Buckets GitHub's `CheckRunState`, `CheckConclusionState` and
/// `StatusState` values the way `gh pr checks` does.
fn bucket(state: &str) -> Bucket {
    match state {
        "SUCCESS" | "COMPLETED" => Bucket::Pass,
        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            Bucket::Fail
        }
        "SKIPPED" | "NEUTRAL" | "STALE" => Bucket::Skipped,
        _ => Bucket::Pending,
    }
}

/// A PR as fetched, with what is still needed to finish paging it.
#[derive(Debug, Clone)]
pub struct Fetched {
    id: String,
    status: PrStatus,
    threads_after: Option<String>,
    rollup_id: Option<String>,
    /// Set while failing checks are still unnamed: the cursor to list them
    /// from, `None` for the first page.
    contexts_after: Option<Option<String>>,
}

impl Fetched {
    pub fn key(&self) -> (&str, u64) {
        (&self.status.repo, self.status.number)
    }

    pub fn is_open(&self) -> bool {
        matches!(self.status.state, State::Open | State::Draft)
    }

    pub fn mergeable_unknown(&self) -> bool {
        self.status.mergeable == Mergeability::Unknown
    }

    pub fn target(&self) -> Option<Target> {
        let repo = Repo::parse(&self.status.repo)?;
        Some(Target::Number(repo, self.status.number))
    }

    /// The follow-up lookups this PR still needs.
    fn pages(&self) -> Vec<Lookup> {
        let mut pages = Vec::new();
        if let Some(after) = &self.threads_after {
            pages.push(Lookup::Threads {
                id: self.id.clone(),
                after: after.clone(),
            });
        }
        if let (Some(id), Some(after)) = (&self.rollup_id, &self.contexts_after) {
            pages.push(Lookup::Contexts {
                id: id.clone(),
                after: after.clone(),
            });
        }
        pages
    }

    fn add_threads(&mut self, threads: RawThreads) {
        let unresolved = threads.nodes.iter().filter(|t| !t.is_resolved).count();
        self.status.unresolved_threads += u32::try_from(unresolved).unwrap_or(u32::MAX);
        self.threads_after = next_cursor(threads.page_info);
    }

    /// Adds failing names from a page of contexts. Pages are fetched only
    /// while some failing check is still unnamed.
    fn add_contexts(&mut self, contexts: RawContexts) {
        let checks = &mut self.status.checks;
        for c in contexts.nodes {
            let (name, state) = match (c.name, c.context) {
                (Some(name), _) => (name, c.conclusion),
                (None, Some(context)) => (context, c.state),
                (None, None) => continue,
            };
            if state.as_deref().map(bucket) == Some(Bucket::Fail) {
                checks.failing.push(name);
            }
        }
        let named_all = checks.failing.len() >= checks.fail as usize;
        self.contexts_after = next_cursor(contexts.page_info)
            .filter(|_| !named_all)
            .map(Some);
    }

    /// The PR as fetched so far, for adding what Entire says.
    pub(crate) fn status_mut(&mut self) -> &mut PrStatus {
        &mut self.status
    }

    pub fn finish(self, now: i64) -> PrStatus {
        let mut status = self.status;
        status.attention = attention(&status, now);
        status
    }
}

fn next_cursor(page: PageInfo) -> Option<String> {
    page.end_cursor.filter(|_| page.has_next_page)
}

/// What `fetch` found.
#[derive(Debug, Default)]
pub struct FetchResult {
    pub prs: Vec<Fetched>,
    /// Targets that named no PR, as `owner/repo#N` or `owner/repo:branch`.
    pub not_found: Vec<String>,
    /// Searches that matched more PRs than GitHub would list.
    pub incomplete: Vec<Incomplete>,
}

/// A search GitHub cut short: it lists at most 1,000 results across all
/// pages, however many match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Incomplete {
    pub query: String,
    pub matched: u64,
    pub listed: u64,
}

/// Fetches every PR the lookups name, following search pages and finishing
/// each PR's thread and check pages. PRs keep the order they were named in,
/// and one named twice is reported once.
pub fn fetch(client: &mut Client, lookups: Vec<Lookup>) -> Result<FetchResult, String> {
    let mut result = FetchResult::default();
    let mut queue = lookups;
    while !queue.is_empty() {
        let batch: Vec<Lookup> = queue.drain(..queue.len().min(BATCH)).collect();
        let (q, vars) = query(&batch);
        let data = client.graphql(&q, &vars)?;
        queue.extend(read_prs(&batch, &data, &mut result)?);
    }
    let mut seen = std::collections::HashSet::new();
    result
        .prs
        .retain(|p| seen.insert((p.status.repo.clone(), p.status.number)));
    let mut seen = std::collections::HashSet::new();
    result.not_found.retain(|t| seen.insert(t.clone()));
    loop {
        let pages: Vec<(usize, Lookup)> = result
            .prs
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.pages().into_iter().map(move |l| (i, l)))
            .collect();
        if pages.is_empty() {
            return Ok(result);
        }
        let (threads, contexts): (Vec<_>, Vec<_>) = pages
            .into_iter()
            .partition(|(_, l)| matches!(l, Lookup::Threads { .. }));
        for batch in threads.chunks(BATCH).chain(contexts.chunks(CONTEXTS_BATCH)) {
            let lookups: Vec<Lookup> = batch.iter().map(|(_, l)| l.clone()).collect();
            let (q, vars) = query(&lookups);
            let data = client.graphql(&q, &vars)?;
            for (i, (pr, lookup)) in batch.iter().enumerate() {
                read_page(&mut result.prs[*pr], lookup, &data[format!("l{i}")])?;
            }
        }
    }
}

/// Re-queries open PRs whose mergeable state is still unknown until GitHub
/// settles it or `settle` runs out.
pub fn settle(client: &mut Client, prs: &mut [Fetched], settle: Duration) -> Result<(), String> {
    let deadline = Instant::now() + settle;
    let mut wait = Duration::from_secs(1);
    let mut announced = false;
    loop {
        let unknown: Vec<Target> = prs
            .iter()
            .filter(|p| p.is_open() && p.mergeable_unknown())
            .filter_map(Fetched::target)
            .collect();
        let left = deadline.saturating_duration_since(Instant::now());
        if unknown.is_empty() || left.is_zero() {
            return Ok(());
        }
        if !announced {
            let names: Vec<String> = unknown.iter().map(Target::to_string).collect();
            eprintln!(
                "jouet: waiting up to {}s for GitHub to compute mergeability of {}",
                left.as_secs(),
                names.join(", ")
            );
            announced = true;
        }
        std::thread::sleep(wait.min(left));
        wait = (wait + Duration::from_secs(1)).min(Duration::from_secs(5));
        let again = fetch(client, unknown.into_iter().map(Lookup::Pr).collect())?;
        for fresh in again.prs {
            if let Some(pr) = prs.iter_mut().find(|p| p.key() == fresh.key()) {
                *pr = fresh;
            }
        }
    }
}

/// Reads the PRs a batch of PR and search lookups returned, and returns
/// the lookups for the next page of each search that has one.
fn read_prs(
    batch: &[Lookup],
    data: &Value,
    result: &mut FetchResult,
) -> Result<Vec<Lookup>, String> {
    let mut more = Vec::new();
    for (i, lookup) in batch.iter().enumerate() {
        let entry = &data[format!("l{i}")];
        match lookup {
            Lookup::Pr(target) => {
                let pr = match target {
                    Target::Number(..) => &entry["pullRequest"],
                    Target::Branch(..) => {
                        let open = &entry["open"]["nodes"][0];
                        if open.is_null() {
                            &entry["newest"]["nodes"][0]
                        } else {
                            open
                        }
                    }
                };
                if pr.is_null() {
                    result.not_found.push(target.to_string());
                } else {
                    result.prs.push(read_pr(pr)?);
                }
            }
            Lookup::Search { query, listed, .. } => {
                let search: RawSearch = parse(entry, "search results")?;
                let mut listed = *listed;
                for node in search.nodes {
                    // Issues match `... on PullRequest` as an empty object.
                    if node.as_object().is_some_and(|o| !o.is_empty()) {
                        result.prs.push(read_pr(&node)?);
                    }
                    listed += 1;
                }
                if let Some(after) = next_cursor(search.page_info) {
                    more.push(Lookup::Search {
                        query: query.clone(),
                        after: Some(after),
                        listed,
                    });
                } else if search.issue_count > listed {
                    result.incomplete.push(Incomplete {
                        query: query.clone(),
                        matched: search.issue_count,
                        listed,
                    });
                }
            }
            Lookup::Threads { .. } | Lookup::Contexts { .. } => {}
        }
    }
    Ok(more)
}

fn read_page(pr: &mut Fetched, lookup: &Lookup, entry: &Value) -> Result<(), String> {
    match lookup {
        Lookup::Threads { .. } => pr.add_threads(parse(&entry["reviewThreads"], "review threads")?),
        Lookup::Contexts { .. } => pr.add_contexts(parse(&entry["contexts"], "checks")?),
        Lookup::Pr(_) | Lookup::Search { .. } => {}
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> Result<T, String> {
    T::deserialize(value).map_err(|e| format!("GitHub sent {what} jouet could not read: {e}"))
}

pub(crate) fn read_pr(value: &Value) -> Result<Fetched, String> {
    let raw: RawPr = parse(value, "a PR")?;
    let state = match (raw.state.as_str(), raw.is_draft) {
        ("MERGED", _) => State::Merged,
        ("CLOSED", _) => State::Closed,
        (_, true) => State::Draft,
        _ => State::Open,
    };
    let mergeable = match raw.mergeable.as_str() {
        "MERGEABLE" => Mergeability::Mergeable,
        "CONFLICTING" => Mergeability::Conflicting,
        _ => Mergeability::Unknown,
    };
    let commit = raw.commits.nodes.into_iter().next().map(|n| n.commit);
    // GitHub's API has no push time for a plain push, so this is the later of
    // the head commit's committer date and the last force push.
    let force_pushed = raw
        .timeline_items
        .nodes
        .into_iter()
        .find_map(|n| n.created_at);
    let pushed_at = [
        commit.as_ref().map(|c| c.committed_date.clone()),
        force_pushed,
    ]
    .into_iter()
    .flatten()
    .max_by_key(|t| time::parse_utc(t));
    let gates = commit
        .as_ref()
        .and_then(|c| c.check_suites.as_ref())
        .and_then(|s| s.nodes.last())
        .and_then(|s| s.check_runs.nodes.last())
        .map(|r| {
            Gates::parse(
                &r.status,
                r.conclusion.as_deref(),
                r.title.as_deref(),
                r.summary.as_deref(),
                r.details_url.as_deref(),
            )
        });
    let mut fetched = Fetched {
        id: raw.id,
        status: PrStatus {
            repo: raw.repository.name_with_owner,
            number: raw.number,
            title: raw.title,
            url: raw.url,
            state,
            base: raw.base_ref_name,
            head: raw.head_ref_name,
            head_sha: raw.head_ref_oid.chars().take(10).collect(),
            head_oid: raw.head_ref_oid,
            pushed_at,
            mergeable,
            merge_state: raw.merge_state_status.to_ascii_lowercase(),
            checks: Checks::default(),
            review: raw.review_decision.map(|r| r.to_ascii_lowercase()),
            unresolved_threads: 0,
            auto_merge: raw.auto_merge_request.is_some_and(|a| !a.is_null()),
            entire: gates.map(EntireStatus::from),
            attention: Vec::new(),
        },
        threads_after: None,
        rollup_id: None,
        contexts_after: None,
    };
    fetched.add_threads(raw.review_threads);
    if let Some(rollup) = commit.and_then(|c| c.status_check_rollup) {
        let counts = rollup
            .contexts
            .check_run_counts_by_state
            .iter()
            .chain(&rollup.contexts.status_context_counts_by_state);
        let checks = &mut fetched.status.checks;
        for c in counts {
            match bucket(&c.state) {
                Bucket::Pass => checks.pass += c.count,
                Bucket::Fail => checks.fail += c.count,
                Bucket::Pending => checks.pending += c.count,
                Bucket::Skipped => checks.skipped += c.count,
            }
        }
        fetched.rollup_id = Some(rollup.id);
        if checks.fail > 0 {
            fetched.contexts_after = Some(None);
        }
    }
    Ok(fetched)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeability {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Checks {
    #[serde(skip_serializing_if = "is_zero")]
    pub pass: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub fail: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub pending: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failing: Vec<String>,
}

impl Checks {
    pub fn total(&self) -> u32 {
        self.pass + self.fail + self.pending + self.skipped
    }

    fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrStatus {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: State,
    pub base: String,
    pub head: String,
    pub head_sha: String,
    /// The whole head SHA, which the merge and update-branch endpoints
    /// take as a guard against a push in between.
    #[serde(skip)]
    pub head_oid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pushed_at: Option<String>,
    pub mergeable: Mergeability,
    pub merge_state: String,
    #[serde(skip_serializing_if = "Checks::is_empty")]
    pub checks: Checks,
    /// GitHub's review decision, lowercased: `approved`,
    /// `changes_requested` or `review_required`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub unresolved_threads: u32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_merge: bool,
    /// The Entire Gates check on the head, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entire: Option<EntireStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<Attention>,
}

/// What Entire says about a PR: its gates, from the check, and when the
/// findings gate fails and the `entire` CLI could be asked, the open
/// findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntireStatus {
    #[serde(flatten)]
    pub gates: Gates,
    /// The open, current findings, read when the findings gate fails.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Brief>,
    /// Entire listed more findings than it let jouet read.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub findings_incomplete: bool,
    /// Why `entire` could not say more, when it was asked and failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<Gates> for EntireStatus {
    fn from(gates: Gates) -> Self {
        Self {
            gates,
            findings: Vec::new(),
            findings_incomplete: false,
            error: None,
        }
    }
}

impl EntireStatus {
    /// The next thing to fix: the first open finding with its handle and
    /// location, or the gate's own reason when none could be read.
    pub fn findings_message(&self) -> String {
        let Some(first) = self.findings.first() else {
            return format!("Entire Gates: {}", self.gates.failing_text());
        };
        let mut s = format!(
            "Fix Entire finding [{}] at {}",
            first.handle, first.location
        );
        if let Some(severity) = &first.severity {
            let _ = write!(s, " ({severity})");
        }
        let _ = write!(s, ": {}", first.title);
        let more = self.findings.len() - 1;
        if more > 0 || self.findings_incomplete {
            let _ = write!(
                s,
                "; {more}{} more open",
                if self.findings_incomplete { "+" } else { "" }
            );
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attention {
    pub code: &'static str,
    pub message: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The reasons an open PR needs attention, in a fixed order. Merged and
/// closed PRs need none.
fn attention(pr: &PrStatus, now: i64) -> Vec<Attention> {
    let mut out = Vec::new();
    if !matches!(pr.state, State::Open | State::Draft) {
        return out;
    }
    let mut add = |code, message: String| out.push(Attention { code, message });
    if pr.mergeable == Mergeability::Conflicting {
        add(
            "conflict",
            format!(
                "Conflicts with {}; pull_request workflows will not run until it is rebased",
                pr.base
            ),
        );
    }
    // A failing Entire Gates check is reported by its gates below, not as a
    // failing check of its own.
    let entire = pr.entire.as_ref().filter(|e| e.gates.failed());
    let gates_failing = u32::from(entire.is_some() && pr.checks.failing.iter().any(|c| c == CHECK));
    let failing: Vec<&str> = pr
        .checks
        .failing
        .iter()
        .map(String::as_str)
        .filter(|c| entire.is_none() || *c != CHECK)
        .collect();
    let fail = pr.checks.fail.saturating_sub(gates_failing);
    if fail > 0 {
        add(
            "checks_failing",
            format!("{fail} failing: {}", failing.join(", ")),
        );
    }
    if pr.checks.total() == 0
        && let Some(pushed) = pr.pushed_at.as_deref().and_then(time::parse_utc)
        && now - pushed > CHECKS_GRACE_SECS
    {
        add(
            "checks_missing",
            format!(
                "No checks registered {} after the last push",
                time::age(now - pushed)
            ),
        );
    }
    if pr.review.as_deref() == Some("changes_requested") {
        add("changes_requested", "A reviewer requested changes".into());
    }
    match pr.unresolved_threads {
        0 => {}
        1 => add("unresolved_threads", "1 unresolved review thread".into()),
        n => add(
            "unresolved_threads",
            format!("{n} unresolved review threads"),
        ),
    }
    if let Some(e) = entire {
        if e.gates.fails("findings") {
            add("entire_findings", e.findings_message());
        }
        let known = ["checks", "findings", "up_to_date"];
        let other: Vec<&Gate> = e
            .gates
            .failing()
            .filter(|g| !known.contains(&g.gate.as_str()))
            .collect();
        if !other.is_empty() || e.gates.failing().next().is_none() {
            add(
                "entire_gates",
                format!("Entire Gates: {}", e.gates.failing_text()),
            );
        }
    }
    if pr.merge_state == "behind" {
        add(
            "behind_base",
            format!("Behind {}; update the branch before it can merge", pr.base),
        );
    } else if let Some(e) = entire.filter(|e| e.gates.fails("up_to_date"))
        && pr.mergeable != Mergeability::Conflicting
    {
        // Entire may want the branch current where GitHub does not.
        let reason = e
            .gates
            .failing()
            .find(|g| g.gate == "up_to_date")
            .and_then(|g| g.reason.clone())
            .unwrap_or_else(|| "not up to date".into());
        add(
            "behind_base",
            format!(
                "Entire Gates: {reason}; update the branch from {} before it can merge",
                pr.base
            ),
        );
    }
    if pr.state == State::Draft {
        add("draft", "The PR is a draft".into());
    }
    if pr.mergeable == Mergeability::Unknown {
        add(
            "mergeable_unknown",
            "GitHub has not finished computing mergeability; run again shortly".into(),
        );
    }
    out
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
