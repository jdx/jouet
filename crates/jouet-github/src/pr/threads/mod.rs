//! Review threads: fetching every thread of a PR, selecting the ones a
//! listing asks for, and finding when a commit was pushed.
//!
//! A thread's handle is the database ID of its first comment. That is the
//! number in the web UI's `#discussion_r...` links, and it maps to both the
//! GraphQL thread ID that resolving needs and the comment a reply goes under.
//! Threads come 100 to a page, as do the comments of each thread, and every
//! further page is fetched, so nothing is silently left out.

pub mod condense;
pub mod write;

#[cfg(test)]
mod tests;

use super::target::{Repo, Target};
use crate::Client;
use jouet_output::time;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const COMMENT: &str = "fullDatabaseId url createdAt body author { __typename login }";
const PAGE: &str = "pageInfo { hasNextPage endCursor }";

/// GitHub lists at most this many commits of a PR.
const MAX_COMMITS: usize = 250;

fn thread_fields() -> String {
    format!(
        "id isResolved isOutdated path line startLine originalLine originalStartLine \
         diffSide startDiffSide comments(first: 100) {{ {PAGE} nodes {{ {COMMENT} }} }}"
    )
}

/// The query for a PR's threads, and with `extra` its reviews and
/// conversation comments.
pub(crate) fn query() -> String {
    format!(
        "query($o: String!, $r: String!, $n: Int!, $extra: Boolean!) {{
  viewer {{ login }}
  repository(owner: $o, name: $r) {{ pullRequest(number: $n) {{
    id number url headRefName headRefOid baseRefOid headRepository {{ nameWithOwner }}
    reviewThreads(first: 100) {{ {PAGE} nodes {{ {threads} }} }}
    reviews(first: 100) @include(if: $extra) {{ {PAGE} nodes {{ state {COMMENT} }} }}
    comments(first: 100) @include(if: $extra) {{ {PAGE} nodes {{ {COMMENT} }} }}
  }} }}
}}",
        threads = thread_fields()
    )
}

const BRANCH_QUERY: &str = "query($o: String!, $r: String!, $b: String!) {
  repository(owner: $o, name: $r) {
    open: pullRequests(headRefName: $b, states: OPEN, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { number } }
    newest: pullRequests(headRefName: $b, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { number } }
  }
}";

/// A PR with every review thread, and its reviews and conversation comments
/// when they were asked for.
#[derive(Debug, Clone)]
pub struct PrThreads {
    pub repo: Repo,
    pub number: u64,
    pub(crate) node_id: String,
    pub head_ref: String,
    pub head_oid: String,
    /// The base branch's head, from which the merge base is found.
    pub base_oid: String,
    pub head_repo: Option<Repo>,
    /// The token's user, whose earlier replies a retry must not repeat.
    pub(crate) viewer: String,
    pub(crate) threads: Vec<Thread>,
    /// Review summaries and conversation comments, oldest first.
    pub(crate) others: Vec<Thread>,
    /// Comments left out because GitHub sent neither their ID nor a URL that
    /// holds it. A thread whose first comment is one of them is left out
    /// whole, since that comment's ID is the thread's handle.
    pub unreadable: usize,
}

/// Which side of the diff a thread's lines are on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The base version: lines the PR deleted or kept.
    #[serde(alias = "LEFT")]
    Left,
    /// The head version.
    #[default]
    #[serde(alias = "RIGHT")]
    Right,
}

impl Side {
    fn is_right(&self) -> bool {
        *self == Self::Right
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Thread,
    Review,
    Comment,
}

#[derive(Debug, Clone, Serialize)]
pub struct Thread {
    pub id: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) node_id: String,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The first line of a multi-line comment, when it is on the same side
    /// of the diff as `line`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_line: Option<u32>,
    /// `left` for a comment on the base version's lines (a deleted or
    /// replaced line), whose numbers are not lines of the head's file.
    /// Absent for the head's side.
    #[serde(skip_serializing_if = "Side::is_right")]
    pub side: Side,
    #[serde(skip_serializing_if = "is_false")]
    pub outdated: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub resolved: bool,
    pub author: String,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) is_bot: bool,
    /// A review's state: `approved`, `changes_requested`, `commented`, ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub(crate) url: String,
    pub comments: Vec<Comment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub(crate) id: u64,
    pub author: String,
    pub(crate) created_at: String,
    pub body: String,
    #[serde(skip_serializing_if = "is_false")]
    pub body_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(skip)]
    pub(crate) url: String,
    /// `created_at` in seconds since the epoch.
    #[serde(skip)]
    pub created: i64,
}

/// Lines of the file around a thread's line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Context {
    pub start: u32,
    pub lines: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    id: String,
    number: u64,
    head_ref_name: String,
    head_ref_oid: String,
    base_ref_oid: String,
    head_repository: Option<NameWithOwner>,
    review_threads: Page<RawThread>,
    reviews: Option<Page<RawComment>>,
    comments: Option<Page<RawComment>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    page_info: PageInfo,
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

impl PageInfo {
    fn next(&self) -> Option<String> {
        self.end_cursor.clone().filter(|_| self.has_next_page)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    id: String,
    is_resolved: bool,
    is_outdated: bool,
    path: Option<String>,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    original_start_line: Option<u32>,
    diff_side: Option<Side>,
    start_diff_side: Option<Side>,
    comments: Page<RawComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawComment {
    /// Nullable in GitHub's schema; the URL holds the same ID.
    full_database_id: Option<String>,
    url: String,
    created_at: String,
    body: String,
    author: Option<RawAuthor>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Deserialize)]
struct RawAuthor {
    #[serde(rename = "__typename")]
    typename: String,
    login: String,
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> Result<T, String> {
    T::deserialize(value).map_err(|e| format!("GitHub sent {what} jouet could not read: {e}"))
}

/// Fetches a PR's threads, and with `extra` its reviews and conversation
/// comments. `None` when the repository or PR does not exist.
pub fn fetch(
    client: &mut Client,
    target: &Target,
    extra: bool,
    bots: &[String],
) -> Result<Option<PrThreads>, String> {
    let (repo, number) = match target {
        Target::Number(repo, n) => (repo, *n),
        Target::Branch(repo, branch) => {
            let vars = json!({ "o": repo.owner, "r": repo.name, "b": branch });
            let data = client.graphql(BRANCH_QUERY, &vars)?;
            let entry = &data["repository"];
            let open = &entry["open"]["nodes"][0]["number"];
            let number = if open.is_null() {
                &entry["newest"]["nodes"][0]["number"]
            } else {
                open
            };
            match number.as_u64() {
                Some(n) => (repo, n),
                None => return Ok(None),
            }
        }
    };
    let vars = json!({ "o": repo.owner, "r": repo.name, "n": number, "extra": extra });
    let data = client.graphql(&query(), &vars)?;
    let mut pages = Pages::default();
    let Some(mut pr) = read(repo, &data, bots, &mut pages)? else {
        return Ok(None);
    };
    pages.fetch(client, &mut pr, bots)?;
    pr.others
        .sort_by_key(|t| t.comments.first().map_or(0, |c| c.created));
    Ok(Some(pr))
}

/// Further pages still to fetch: of the PR's threads, reviews and comments
/// (by the PR's node ID), and of each thread's comments (by thread index).
#[derive(Default)]
struct Pages {
    threads: Option<String>,
    reviews: Option<String>,
    comments: Option<String>,
    thread_comments: Vec<(usize, String)>,
}

impl Pages {
    fn fetch(
        &mut self,
        client: &mut Client,
        pr: &mut PrThreads,
        bots: &[String],
    ) -> Result<(), String> {
        let threads = thread_fields();
        let reviews = format!("state {COMMENT}");
        loop {
            let (field, nodes, after) = if let Some(after) = self.threads.take() {
                ("reviewThreads", threads.as_str(), after)
            } else if let Some(after) = self.reviews.take() {
                ("reviews", reviews.as_str(), after)
            } else if let Some(after) = self.comments.take() {
                ("comments", COMMENT, after)
            } else {
                break;
            };
            let q = format!(
                "query($id: ID!, $c: String!) {{ node(id: $id) {{ ... on PullRequest {{ \
                 {field}(first: 100, after: $c) {{ {PAGE} nodes {{ {nodes} }} }} }} }} }}"
            );
            let data = client.graphql(&q, &json!({ "id": pr.node_id, "c": after }))?;
            let entry = &data["node"][field];
            match field {
                "reviewThreads" => {
                    let page: Page<RawThread> = parse(entry, "review threads")?;
                    self.threads = page.page_info.next();
                    self.add_threads(pr, page.nodes, bots);
                }
                _ => {
                    let page: Page<RawComment> = parse(entry, "comments")?;
                    let kind = if field == "reviews" {
                        self.reviews = page.page_info.next();
                        Kind::Review
                    } else {
                        self.comments = page.page_info.next();
                        Kind::Comment
                    };
                    add_others(pr, kind, page.nodes, bots);
                }
            }
        }
        while let Some((i, after)) = self.thread_comments.pop() {
            let q = format!(
                "query($id: ID!, $c: String!) {{ node(id: $id) {{ ... on PullRequestReviewThread {{ \
                 comments(first: 100, after: $c) {{ {PAGE} nodes {{ {COMMENT} }} }} }} }} }}"
            );
            let thread = &mut pr.threads[i];
            let data = client.graphql(&q, &json!({ "id": thread.node_id, "c": after }))?;
            let page: Page<RawComment> = parse(&data["node"]["comments"], "thread comments")?;
            for raw in page.nodes {
                match comment(raw) {
                    Some(c) => thread.comments.push(c),
                    None => pr.unreadable += 1,
                }
            }
            if let Some(next) = page.page_info.next() {
                self.thread_comments.push((i, next));
            }
        }
        Ok(())
    }

    fn add_threads(&mut self, pr: &mut PrThreads, raw: Vec<RawThread>, bots: &[String]) {
        for raw in raw {
            let total = raw.comments.nodes.len();
            let mut nodes = raw.comments.nodes.into_iter();
            // A thread whose comments were all deleted has no handle.
            let Some(first) = nodes.next() else {
                continue;
            };
            let Some(first) = comment(first) else {
                pr.unreadable += total;
                continue;
            };
            let mut comments = vec![first];
            for raw in nodes {
                match comment(raw) {
                    Some(c) => comments.push(c),
                    None => pr.unreadable += 1,
                }
            }
            if let Some(next) = raw.comments.page_info.next() {
                self.thread_comments.push((pr.threads.len(), next));
            }
            let first = &comments[0];
            pr.threads.push(Thread {
                id: first.id,
                node_id: raw.id,
                kind: Kind::Thread,
                path: raw.path,
                line: raw.line,
                // A range that starts on the other side of the diff has no
                // first line in `line`'s file.
                start_line: raw
                    .start_line
                    .or(raw.original_start_line.filter(|_| raw.line.is_none()))
                    .filter(|_| raw.start_diff_side.is_none_or(|s| Some(s) == raw.diff_side)),
                original_line: raw.original_line,
                side: raw.diff_side.unwrap_or_default(),
                outdated: raw.is_outdated,
                resolved: raw.is_resolved,
                is_bot: is_bot(&first.author, bots),
                author: first.author.clone(),
                state: None,
                url: first.url.clone(),
                comments,
                context: None,
            });
        }
    }
}

fn read(
    repo: &Repo,
    data: &Value,
    bots: &[String],
    pages: &mut Pages,
) -> Result<Option<PrThreads>, String> {
    let entry = &data["repository"]["pullRequest"];
    if entry.is_null() {
        return Ok(None);
    }
    let raw: RawPr = parse(entry, "a PR")?;
    let mut pr = PrThreads {
        repo: repo.clone(),
        number: raw.number,
        node_id: raw.id,
        head_ref: raw.head_ref_name,
        head_oid: raw.head_ref_oid,
        base_oid: raw.base_ref_oid,
        head_repo: raw
            .head_repository
            .and_then(|r| Repo::parse(&r.name_with_owner)),
        viewer: data["viewer"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        threads: Vec::new(),
        others: Vec::new(),
        unreadable: 0,
    };
    pages.threads = raw.review_threads.page_info.next();
    pages.add_threads(&mut pr, raw.review_threads.nodes, bots);
    if let Some(reviews) = raw.reviews {
        pages.reviews = reviews.page_info.next();
        add_others(&mut pr, Kind::Review, reviews.nodes, bots);
    }
    if let Some(comments) = raw.comments {
        pages.comments = comments.page_info.next();
        add_others(&mut pr, Kind::Comment, comments.nodes, bots);
    }
    Ok(Some(pr))
}

/// Adds review summaries or conversation comments; reviews with no summary
/// (a bare batch of line comments) are left out.
fn add_others(pr: &mut PrThreads, kind: Kind, raw: Vec<RawComment>, bots: &[String]) {
    for raw in raw {
        if raw.body.trim().is_empty() {
            continue;
        }
        let state = raw.state.as_deref().map(str::to_ascii_lowercase);
        let Some(c) = comment(raw) else {
            pr.unreadable += 1;
            continue;
        };
        pr.others.push(Thread {
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
            is_bot: is_bot(&c.author, bots),
            author: c.author.clone(),
            state,
            url: c.url.clone(),
            comments: vec![c],
            context: None,
        });
    }
}

/// A comment, or `None` when GitHub sent no ID jouet can read, in
/// `fullDatabaseId` or in the URL.
fn comment(raw: RawComment) -> Option<Comment> {
    let id = raw
        .full_database_id
        .as_deref()
        .and_then(|id| id.parse().ok())
        .or_else(|| url_id(&raw.url))?;
    let author = match raw.author {
        // GraphQL names an app `coderabbitai`; REST and the web UI say
        // `coderabbitai[bot]`, which is what people filter by.
        Some(a) if a.typename == "Bot" && !a.login.ends_with("[bot]") => {
            format!("{}[bot]", a.login)
        }
        Some(a) => a.login,
        None => "ghost".into(),
    };
    Some(Comment {
        id,
        author,
        created: time::parse_utc(&raw.created_at).unwrap_or(0),
        created_at: raw.created_at,
        body: raw.body,
        body_truncated: false,
        suggestion: None,
        url: raw.url,
    })
}

/// The database ID at the end of a comment's URL: `#discussion_r55500`,
/// `#issuecomment-66601` or `#pullrequestreview-4401`.
fn url_id(url: &str) -> Option<u64> {
    let (_, fragment) = url.rsplit_once('#')?;
    fragment
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()
}

pub(crate) fn is_bot(login: &str, bots: &[String]) -> bool {
    login.ends_with("[bot]") || bots.iter().any(|b| b.eq_ignore_ascii_case(login))
}

/// Whether `login` names `author`, ignoring case and a missing `[bot]`.
pub(crate) fn same_login(login: &str, author: &str) -> bool {
    let bare = |s: &str| s.strip_suffix("[bot]").unwrap_or(s).to_ascii_lowercase();
    bare(login) == bare(author)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unresolved,
    Resolved,
    All,
}

/// Which threads a listing shows. `None` means "either".
#[derive(Debug, Clone)]
pub struct Filter {
    pub status: Status,
    pub outdated: Option<bool>,
    pub authors: Vec<String>,
    pub bots: Option<bool>,
    pub since: Option<i64>,
    pub paths: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub unresolved: usize,
    pub resolved: usize,
    /// Unresolved threads whose line has since changed.
    pub outdated: usize,
}

pub fn counts(pr: &PrThreads) -> Counts {
    let mut c = Counts::default();
    for t in &pr.threads {
        if t.resolved {
            c.resolved += 1;
        } else {
            c.unresolved += 1;
            c.outdated += usize::from(t.outdated);
        }
    }
    c
}

/// The threads (then reviews and comments) a filter keeps, in order.
pub fn select<'a>(pr: &'a PrThreads, f: &Filter) -> Vec<&'a Thread> {
    let keep = |t: &&Thread| {
        let status = t.kind != Kind::Thread
            || match f.status {
                Status::Unresolved => !t.resolved,
                Status::Resolved => t.resolved,
                Status::All => true,
            };
        let outdated = f.outdated.is_none_or(|o| o == t.outdated);
        let author = f.authors.is_empty() || f.authors.iter().any(|a| same_login(a, &t.author));
        let bots = f.bots.is_none_or(|b| b == t.is_bot);
        let since = f
            .since
            .is_none_or(|since| t.comments.iter().any(|c| c.created >= since));
        let path = f.paths.is_empty()
            || t.path
                .as_deref()
                .is_some_and(|p| f.paths.iter().any(|g| path_matches(g, p)));
        status && outdated && author && bots && since && path
    };
    pr.threads.iter().chain(&pr.others).filter(keep).collect()
}

/// Whether `path` matches a `--path` pattern: a glob over the whole path
/// (`*` and `?` stay within a directory, `**` crosses them), a directory
/// holding it, or, for a pattern without `/`, its file name.
pub(crate) fn path_matches(pattern: &str, path: &str) -> bool {
    let dir = pattern.trim_end_matches('/');
    if path
        .strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
    {
        return true;
    }
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return glob(pattern.as_bytes(), name.as_bytes());
    }
    glob(pattern.as_bytes(), path.as_bytes())
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    match p {
        [] => s.is_empty(),
        [b'*', b'*'] => true,
        [b'*', b'*', rest @ ..] => {
            // `**/` matches no directories, or any number of whole ones.
            let rest = rest.strip_prefix(b"/").unwrap_or(rest);
            glob(rest, s)
                || s.iter()
                    .enumerate()
                    .any(|(i, &c)| c == b'/' && glob(rest, &s[i + 1..]))
        }
        [b'*', rest @ ..] => (0..=s.len())
            .take_while(|&i| i == 0 || s[i - 1] != b'/')
            .any(|i| glob(rest, &s[i..])),
        [b'?', rest @ ..] => s.first().is_some_and(|&c| c != b'/') && glob(rest, &s[1..]),
        [c, rest @ ..] => s.first() == Some(c) && glob(rest, &s[1..]),
    }
}

/// One entry of GitHub's repository activity log.
#[derive(Debug, Deserialize)]
pub(crate) struct Activity {
    pub(crate) after: String,
    pub(crate) timestamp: String,
}

/// The commit an `@SHA` names, and when it was pushed to the PR.
#[derive(Debug, PartialEq, Eq)]
pub struct Pushed {
    pub oid: String,
    pub at: i64,
    /// Set when the activity log had no push of it, so `at` is the
    /// commit's own time: earlier than the push, so nothing is missed.
    pub estimated: bool,
}

/// Finds when `sha` (a prefix of at least 4 characters) was pushed to the
/// PR: one of its commits, or a head it had before a force-push. `Err` says
/// why `sha` names no such commit.
pub fn pushed_at(client: &mut Client, pr: &PrThreads, sha: &str) -> Result<Pushed, Lookup> {
    let sha = sha_prefix(sha)?;
    let commits = commits(client, pr).map_err(Lookup::Remote)?;
    let repo = pr.head_repo.as_ref().unwrap_or(&pr.repo);
    let path = format!(
        "/repos/{repo}/activity?ref={}&per_page=100",
        encode(&format!("refs/heads/{}", pr.head_ref))
    );
    let activity: Vec<Activity> = match client.rest_get(&path) {
        Ok(Some(v)) => parse(&v, "repository activity").unwrap_or_default(),
        Ok(None) => Vec::new(),
        Err(e) => {
            eprintln!("jouet: could not read the push log of {repo} ({e})");
            Vec::new()
        }
    };
    pushed_from(&commits, &activity, &sha)
}

/// `sha` lowercased, when it is at least 4 hex digits.
pub(crate) fn sha_prefix(sha: &str) -> Result<String, Lookup> {
    let sha = sha.to_ascii_lowercase();
    if sha.len() < 4 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Lookup::Usage(format!(
            "`{sha}` is not a commit SHA (give at least 4 hex digits)"
        )));
    }
    Ok(sha)
}

/// When the commit `sha` (a lowercase prefix) names reached the PR, from
/// its commits (oldest first, with their commit times) and a log of pushes.
pub(crate) fn pushed_from(
    commits: &[(String, i64)],
    activity: &[Activity],
    sha: &str,
) -> Result<Pushed, Lookup> {
    let oid = find_commit(commits, activity, sha)?;
    let oids: Vec<&str> = commits.iter().map(|(o, _)| o.as_str()).collect();
    Ok(match push_time(activity, &oids, &oid) {
        Some(at) => Pushed {
            oid,
            at,
            estimated: false,
        },
        None => {
            // Only a PR commit can be missing from the log.
            let at = commits
                .iter()
                .find(|(o, _)| *o == oid)
                .map_or(0, |(_, at)| *at);
            Pushed {
                oid,
                at,
                estimated: true,
            }
        }
    })
}

/// Why an `@SHA` could not be used.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    NotFound(String),
    /// Not a SHA, or one naming more than one commit.
    Usage(String),
    Remote(String),
}

/// The PR's commits, oldest first, with their commit times.
fn commits(client: &mut Client, pr: &PrThreads) -> Result<Vec<(String, i64)>, String> {
    let q = "query($id: ID!, $c: String) { node(id: $id) { ... on PullRequest { \
             commits(first: 100, after: $c) { pageInfo { hasNextPage endCursor } \
             nodes { commit { oid committedDate } } } } } }";
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let data = client.graphql(q, &json!({ "id": pr.node_id, "c": after }))?;
        let entry = &data["node"]["commits"];
        for node in entry["nodes"].as_array().into_iter().flatten() {
            let commit = &node["commit"];
            let oid = commit["oid"].as_str().unwrap_or_default().to_string();
            let at = commit["committedDate"]
                .as_str()
                .and_then(time::parse_utc)
                .unwrap_or(0);
            out.push((oid, at));
        }
        after = entry["pageInfo"]["endCursor"]
            .as_str()
            .filter(|_| entry["pageInfo"]["hasNextPage"].as_bool() == Some(true))
            .map(str::to_string);
        if after.is_none() || out.len() >= MAX_COMMITS {
            return Ok(out);
        }
    }
}

/// The one commit `sha` starts: a commit of the PR, or a head the push log
/// shows it had. `sha` is lowercase hex.
fn find_commit(
    commits: &[(String, i64)],
    activity: &[Activity],
    sha: &str,
) -> Result<String, Lookup> {
    let mut found: Vec<&str> = commits
        .iter()
        .map(|(o, _)| o.as_str())
        .chain(activity.iter().map(|a| a.after.as_str()))
        .filter(|o| o.starts_with(sha))
        .collect();
    found.sort_unstable();
    found.dedup();
    match found.as_slice() {
        [one] => Ok((*one).to_string()),
        [] => Err(Lookup::NotFound(format!(
            "{sha} is not a commit of this PR, nor a head it was pushed with"
        ))),
        _ => Err(Lookup::Usage(format!(
            "{sha} matches {} commits of this PR; give more of the SHA",
            found.len()
        ))),
    }
}

/// When `oid` reached the branch: the earliest push whose new head is `oid`
/// or a later commit of the PR. `commits` is the PR's commits, oldest first.
pub(crate) fn push_time(activity: &[Activity], commits: &[&str], oid: &str) -> Option<i64> {
    let index = commits.iter().position(|c| *c == oid);
    activity
        .iter()
        .filter(|a| {
            a.after == oid
                || index.is_some_and(|i| {
                    commits
                        .iter()
                        .position(|c| *c == a.after)
                        .is_some_and(|j| j >= i)
                })
        })
        .filter_map(|a| time::parse_utc(&a.timestamp))
        .min()
}

/// Percent-encodes a query-string value.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
