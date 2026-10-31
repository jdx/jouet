//! `jouet pr threads`: listing review threads, and replying to and resolving
//! them.

use super::Output;
use super::entire::{self, Wanted};
use crate::config::Config;
use crate::{ApplyArgs, ReplyArgs, ResolveArgs, ThreadsArgs, ThreadsCmd, UnresolveArgs, git};
use jouet_github::entire::Change;
use jouet_github::host::{self, Feature, Host};
use jouet_github::pr::target::{self, Arg, Repo, Target};
use jouet_github::pr::threads::condense::{self, condense};
use jouet_github::pr::threads::write::{self, Entry, Options, Outcome};
use jouet_github::pr::threads::{
    self, Context, Counts, Filter, Kind, Lookup, PrThreads, Side, Status, Thread,
};
use jouet_output::exit::Exit;
use jouet_output::time;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{IsTerminal, Read};

/// Lines a condensed comment keeps.
const MAX_LINES: usize = 30;

/// A failure that stops a command: its exit code, message and hint.
pub(super) struct Stop(
    pub(super) Exit,
    pub(super) String,
    pub(super) Option<String>,
);

impl Stop {
    pub(super) fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self(exit, message.into(), None)
    }
}

pub(crate) fn run(args: ThreadsArgs, out: &Output) -> Exit {
    let result = match &args.cmd {
        None => list(&args, out),
        Some(ThreadsCmd::Reply(a)) => reply(&args, a, out),
        Some(ThreadsCmd::Resolve(a)) => resolve(&args, a, out),
        Some(ThreadsCmd::Unresolve(a)) => unresolve(&args, a, out),
        Some(ThreadsCmd::Apply(a)) => apply(&args, a, out),
    };
    result.unwrap_or_else(|Stop(exit, message, hint)| {
        out.error_with_hint(exit, &message, hint.as_deref())
    })
}

/// The PR an argument names, or the current branch's.
fn target(pr: Option<&str>, repo: Option<&str>) -> Result<Target, Stop> {
    let usage = |m: String| Stop::new(Exit::Usage, m);
    let repo = match repo {
        Some(r) => Some(Repo::parse(r).ok_or_else(|| {
            usage(format!(
                "--repo takes OWNER/NAME or HOST/OWNER/NAME, not `{r}`"
            ))
        })?),
        None => None,
    };
    let repo_for = |what: &str| {
        repo.clone().or_else(target::repo_from_git).ok_or_else(|| {
            usage(format!(
                "no --repo given and no GitHub remote here, so {what} names no repository"
            ))
        })
    };
    Ok(match pr {
        Some(pr) => match Arg::parse(pr) {
            Arg::Number(n) => Target::Number(repo_for(&format!("`{pr}`"))?, n),
            Arg::Branch(branch) => Target::Branch(repo_for(&format!("`{pr}`"))?, branch),
            Arg::InRepo(repo, n) => Target::Number(repo, n),
        },
        None => {
            let branch = target::current_branch()
                .ok_or_else(|| usage("not on a branch: name the PR".into()))?;
            Target::Branch(repo_for("the current branch")?, branch)
        }
    })
}

fn config() -> Result<Config, Stop> {
    Config::load().map_err(|e| Stop::new(Exit::Usage, e))
}

/// The host the PR is on.
fn connect(target: &Target) -> Result<Box<dyn Host>, Stop> {
    host::connect(target.repo()).map_err(|e| Stop::new(Exit::Precondition, e))
}

fn fetch(
    host: &mut dyn Host,
    target: &Target,
    extra: bool,
    bots: &[String],
) -> Result<PrThreads, Stop> {
    host.threads(target, extra, bots)
        .map_err(|e| Stop::new(Exit::Remote, e))?
        .ok_or_else(|| Stop::new(Exit::NotFound, format!("no PR {target}")))
}

#[derive(Serialize)]
struct Listing<'a> {
    repo: String,
    pr: u64,
    counts: Counts,
    /// The `--since` cutoff, in UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
    threads: Vec<Thread>,
    /// Comments left out because GitHub sent no ID for them.
    #[serde(skip_serializing_if = "is_zero")]
    unreadable: usize,
    /// The findings on the PR's Entire trail, apart from the threads.
    entire: entire::Listing,
    #[serde(skip)]
    since_label: Option<&'a str>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn list(args: &ThreadsArgs, out: &Output) -> Result<Exit, Stop> {
    let target = target(args.pr.as_deref(), args.repo.as_deref())?;
    let config = config()?;
    let now = time::now();
    let since_time = match args.since.as_deref() {
        Some(s) if !s.starts_with('@') => Some(
            time::parse_time(s, now)
                .map_err(|e| Stop::new(Exit::Usage, format!("--since: {e}")))?,
        ),
        _ => None,
    };
    let probe = entire::Probe::start();
    let mut host = connect(&target)?;
    let bots = config.bots();
    let pr = fetch(host.as_mut(), &target, args.include_reviews, &bots)?;
    let since = match args.since.as_deref().and_then(|s| s.strip_prefix('@')) {
        Some(sha) => Some(push_time(host.as_mut(), &pr, sha)?),
        None => since_time,
    };
    let filter = Filter {
        status: if args.all {
            Status::All
        } else if args.resolved {
            Status::Resolved
        } else {
            Status::Unresolved
        },
        outdated: flag(args.outdated, args.no_outdated),
        authors: args.author.clone(),
        bots: flag(args.bots, args.no_bots),
        since,
        paths: args.path.clone(),
    };
    let mut shown: Vec<Thread> = threads::select(&pr, &filter).into_iter().cloned().collect();
    for comment in shown.iter_mut().flat_map(|t| &mut t.comments) {
        comment.suggestion = condense::suggestion(&comment.body);
        if !args.full {
            let c = condense(&comment.body, MAX_LINES);
            comment.body = c.body;
            comment.body_truncated = c.truncated;
        }
    }
    if let Some(n) = args.context {
        add_context(host.as_mut(), &pr, &mut shown, n);
    }
    let unresolved = shown.iter().any(|t| t.kind == Kind::Thread && !t.resolved);
    let entire = entire::list(probe, &pr.repo, &pr.head_ref, &filter, args.full, MAX_LINES);
    let listing = Listing {
        repo: pr.repo.to_string(),
        pr: pr.number,
        counts: threads::counts(&pr),
        since: since.map(time::format_utc),
        threads: shown,
        unreadable: pr.unreadable,
        entire,
        since_label: args.since.as_deref(),
    };
    out.result(&listing, || render_list(&listing, now));
    Ok(if args.fail_if_unresolved && unresolved {
        Exit::Failed
    } else {
        Exit::Ok
    })
}

fn flag(yes: bool, no: bool) -> Option<bool> {
    match (yes, no) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

fn push_time(host: &mut dyn Host, pr: &PrThreads, sha: &str) -> Result<i64, Stop> {
    let pushed = host.pushed_at(pr, sha).map_err(|e| match e {
        Lookup::NotFound(m) => Stop::new(Exit::NotFound, format!("--since @{sha}: {m}")),
        Lookup::Usage(m) => Stop::new(Exit::Usage, format!("--since @{sha}: {m}")),
        Lookup::Remote(m) => Stop::new(Exit::Remote, m),
    })?;
    if pushed.estimated {
        eprintln!(
            "jouet: found no push of {} in {}'s push log; using its commit time, which can only be earlier",
            &pushed.oid[..pushed.oid.len().min(7)],
            host.forge().name()
        );
    }
    Ok(pushed.at)
}

/// Adds lines of the file around each current thread's line. A thread on
/// the head's side reads the working tree when HEAD is the PR's head and the
/// file is unchanged, else the head from the forge. A thread on the base's
/// side (a deleted or replaced line) reads the merge base, which is what the
/// diff shows on that side. A file that cannot be read gets no context.
fn add_context(host: &mut dyn Host, pr: &PrThreads, shown: &mut [Thread], n: u32) {
    let paths = |side: Side| {
        let mut paths: Vec<String> = shown
            .iter()
            .filter(|t| t.line.is_some() && t.side == side)
            .filter_map(|t| t.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    };
    let (right, left) = (paths(Side::Right), paths(Side::Left));
    let mut files: HashMap<(Side, String), String> = HashMap::new();
    let local = git::toplevel().filter(|_| git::head().as_deref() == Some(pr.head_oid.as_str()));
    let mut remote = Vec::new();
    for path in right {
        match local.as_ref().filter(|top| git::unchanged(top, &path)) {
            Some(top) => {
                if let Ok(text) = std::fs::read_to_string(top.join(&path)) {
                    files.insert((Side::Right, path), text);
                }
            }
            None => remote.push(path),
        }
    }
    fetch_files(host, pr, &pr.head_oid, Side::Right, &remote, &mut files);
    if !left.is_empty() {
        match host.merge_base(pr) {
            Ok(oid) => fetch_files(host, pr, &oid, Side::Left, &left, &mut files),
            Err(e) => eprintln!("jouet: could not find the PR's merge base for --context ({e})"),
        }
    }
    for t in shown {
        let (Some(path), Some(line)) = (&t.path, t.line) else {
            continue;
        };
        if let Some(text) = files.get(&(t.side, path.clone())) {
            t.context = context(text, t.start_line.unwrap_or(line), line, n);
        }
    }
}

/// Adds `paths` at commit `oid`, for threads on `side`, to `files`.
fn fetch_files(
    host: &mut dyn Host,
    pr: &PrThreads,
    oid: &str,
    side: Side,
    paths: &[String],
    files: &mut HashMap<(Side, String), String>,
) {
    if paths.is_empty() {
        return;
    }
    match host.files(pr, oid, paths) {
        Ok(fetched) => files.extend(fetched.into_iter().map(|(p, text)| ((side, p), text))),
        Err(e) => eprintln!("jouet: could not fetch files for --context ({e})"),
    }
}

/// Lines `start - n ..= end + n` of `text`, clamped to the file.
fn context(text: &str, start: u32, end: u32, n: u32) -> Option<Context> {
    let lines: Vec<&str> = text.lines().collect();
    let first = start.saturating_sub(n).max(1);
    let last = end
        .saturating_add(n)
        .min(u32::try_from(lines.len()).unwrap_or(u32::MAX));
    if first > last {
        return None;
    }
    Some(Context {
        start: first,
        lines: (first..=last)
            .map(|i| lines[i as usize - 1].to_string())
            .collect(),
    })
}

fn render_list(l: &Listing, now: i64) -> String {
    let c = &l.counts;
    let total = c.unresolved + c.resolved;
    let mut s = format!(
        "{}#{}  {} unresolved of {total} {}",
        l.repo,
        l.pr,
        c.unresolved,
        if total == 1 { "thread" } else { "threads" }
    );
    if c.outdated > 0 {
        let _ = write!(s, " ({} outdated)", c.outdated);
    }
    let threads = l.threads.iter().filter(|t| t.kind == Kind::Thread).count();
    let others = l.threads.len() - threads;
    if threads != c.unresolved || l.threads.iter().any(|t| t.resolved) {
        let _ = write!(s, "  showing {threads}");
    }
    if others > 0 {
        let _ = write!(s, "  +{others} reviews and comments");
    }
    if let Some(since) = l.since_label {
        let _ = write!(s, "  since {since}");
    }
    s.push('\n');
    if l.unreadable > 0 {
        let _ = writeln!(
            s,
            "{} {} left out: GitHub sent no ID for {}",
            l.unreadable,
            if l.unreadable == 1 {
                "comment"
            } else {
                "comments"
            },
            if l.unreadable == 1 { "it" } else { "them" },
        );
    }
    for t in &l.threads {
        s.push('\n');
        render_thread(&mut s, t, now);
    }
    // Where the IDs are is where an agent decides to reply, so the usage is
    // here rather than one `--help` away.
    if l.threads
        .iter()
        .any(|t| t.kind == Kind::Thread && !t.resolved)
        && let Some(reply) = crate::guide::short_synopsis(&["pr", "threads", "reply"], &["--body"])
    {
        let _ = writeln!(
            s,
            "\nReply with `{reply}`, ID being the number in brackets; add --resolve to resolve it too, or `jouet pr threads apply` takes many at once."
        );
    }
    entire::render(&mut s, &l.entire, now);
    s
}

fn render_thread(s: &mut String, t: &Thread, now: i64) {
    let first = t.comments.first();
    let age = |c: &threads::Comment| format!("{} ago", time::age(now - c.created));
    let _ = write!(s, "[{}] ", t.id);
    match t.kind {
        Kind::Thread => {
            s.push_str(t.path.as_deref().unwrap_or("?"));
            if let Some(line) = t.line.or(t.original_line) {
                match t.start_line.filter(|&start| start < line) {
                    Some(start) => {
                        let _ = write!(s, ":{start}-{line}");
                    }
                    None => {
                        let _ = write!(s, ":{line}");
                    }
                }
            }
        }
        Kind::Review => s.push_str("review"),
        Kind::Comment => s.push_str("comment"),
    }
    let _ = write!(s, "  {}", t.author);
    if let Some(c) = first {
        let _ = write!(s, "  {}", age(c));
    }
    if t.comments.len() > 1 {
        let _ = write!(s, "  {} comments", t.comments.len());
    }
    for (on, word) in [
        (t.side == Side::Left, "base side"),
        (t.outdated, "outdated"),
        (t.resolved, "resolved"),
    ] {
        if on {
            let _ = write!(s, "  {word}");
        }
    }
    if let Some(state) = t.state.as_deref().filter(|&st| st != "commented") {
        let _ = write!(s, "  {state}");
    }
    s.push('\n');
    if let Some(ctx) = &t.context {
        let last = ctx.start as usize + ctx.lines.len() - 1;
        let width = last.to_string().len();
        let line = t.line.unwrap_or(0);
        let start = t.start_line.unwrap_or(line);
        for (i, text) in ctx.lines.iter().enumerate() {
            let n = ctx.start + u32::try_from(i).unwrap_or(u32::MAX);
            let mark = if (start..=line).contains(&n) {
                '>'
            } else {
                ' '
            };
            let _ = writeln!(s, "  {mark}{n:>width$} | {text}");
        }
    }
    for (i, c) in t.comments.iter().enumerate() {
        if i == 0 {
            body_lines(s, &c.body, "  ", "  ");
        } else {
            let lead = format!("  > {} ({}): ", c.author, age(c));
            body_lines(s, &c.body, &lead, "    ");
        }
    }
}

/// Writes a body: its first line after `lead`, the rest after `indent`, and
/// `suggestion:` before each suggestion block.
fn body_lines(s: &mut String, body: &str, lead: &str, indent: &str) {
    if body.is_empty() {
        let _ = writeln!(s, "{}", lead.trim_end());
        return;
    }
    let mut prefix = lead;
    let mut fence: Option<String> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if fence.is_none() && trimmed.starts_with("```suggestion") {
            let _ = writeln!(s, "{prefix}suggestion:");
            prefix = indent;
        }
        if let Some(open) = &fence {
            if trimmed.starts_with(open.as_str()) && trimmed.trim_end_matches('`').is_empty() {
                fence = None;
            }
        } else if let Some(run) = trimmed
            .strip_prefix("```")
            .map(|r| 3 + r.chars().take_while(|&c| c == '`').count())
        {
            fence = Some("`".repeat(run));
        }
        if line.is_empty() {
            s.push('\n');
        } else {
            let _ = writeln!(s, "{prefix}{line}");
        }
        prefix = indent;
    }
}

fn reply(args: &ThreadsArgs, a: &ReplyArgs, out: &Output) -> Result<Exit, Stop> {
    let finding = entire::handle(&a.id).is_some();
    if a.dismiss && !finding {
        return Err(Stop::new(
            Exit::Usage,
            "--dismiss is for Entire findings (entire:ID); a GitHub thread is resolved",
        ));
    }
    let id = if finding { 0 } else { id(&a.id)? };
    let body = match (&a.body, &a.body_file) {
        (Some(body), None) => body.clone(),
        (None, Some(file)) => read_input(file, "--body-file")?,
        _ => {
            return Err(Stop::new(
                Exit::Usage,
                "give the reply as --body TEXT or --body-file FILE",
            ));
        }
    };
    if body.trim().is_empty() {
        return Err(Stop::new(
            Exit::Usage,
            "the reply is empty; nothing was posted",
        ));
    }
    let pr = a.pr.as_deref().or(args.pr.as_deref());
    if finding {
        let change = match (a.dismiss, a.resolve) {
            (true, _) => Change::Dismiss,
            (false, true) => Change::Resolve,
            (false, false) => {
                return Err(Stop::new(
                    Exit::Usage,
                    format!(
                        "{} is an Entire finding, which takes a reason only when resolved or dismissed: add --resolve, or --resolve --dismiss; nothing was changed",
                        a.id
                    ),
                ));
            }
        };
        let wanted = [Wanted::new(&a.id, change, Some(body))?];
        return write_threads(args, pr, &[], &wanted, a.ignore_resolved, a.dry_run, out);
    }
    let entries = vec![Entry {
        id,
        reply: Some(body),
        resolve: a.resolve,
        unresolve: false,
    }];
    write_threads(args, pr, &entries, &[], a.ignore_resolved, a.dry_run, out)
}

fn resolve(args: &ThreadsArgs, a: &ResolveArgs, out: &Output) -> Result<Exit, Stop> {
    let Named { ids, handles, pr } = ids_and_pr(&a.args)?;
    if a.reply.as_deref().is_some_and(|r| r.trim().is_empty()) {
        return Err(Stop::new(
            Exit::Usage,
            "--reply is empty; nothing was posted",
        ));
    }
    if a.dismiss && !ids.is_empty() {
        return Err(Stop::new(
            Exit::Usage,
            "--dismiss is for Entire findings (entire:ID); GitHub threads are resolved",
        ));
    }
    let change = if a.dismiss {
        Change::Dismiss
    } else {
        Change::Resolve
    };
    let wanted = handles
        .iter()
        .map(|h| Wanted::new(h, change, a.reply.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let entries: Vec<Entry> = ids
        .into_iter()
        .map(|id| Entry {
            id,
            reply: a.reply.clone(),
            resolve: true,
            unresolve: false,
        })
        .collect();
    let pr = pr.or(args.pr.as_deref());
    write_threads(
        args,
        pr,
        &entries,
        &wanted,
        a.ignore_resolved,
        a.dry_run,
        out,
    )
}

fn unresolve(args: &ThreadsArgs, a: &UnresolveArgs, out: &Output) -> Result<Exit, Stop> {
    let Named { ids, handles, pr } = ids_and_pr(&a.args)?;
    let wanted = handles
        .iter()
        .map(|h| Wanted::new(h, Change::Reopen, None))
        .collect::<Result<Vec<_>, _>>()?;
    let entries: Vec<Entry> = ids
        .into_iter()
        .map(|id| Entry {
            id,
            reply: None,
            resolve: false,
            unresolve: true,
        })
        .collect();
    let pr = pr.or(args.pr.as_deref());
    write_threads(args, pr, &entries, &wanted, false, a.dry_run, out)
}

fn apply(args: &ThreadsArgs, a: &ApplyArgs, out: &Output) -> Result<Exit, Stop> {
    let stdin = a.args.iter().any(|s| s == "-");
    let prs: Vec<&String> = a.args.iter().filter(|s| *s != "-").collect();
    if prs.len() > 1 {
        return Err(Stop::new(Exit::Usage, "apply takes at most one PR"));
    }
    let text = match (&a.file, stdin) {
        (Some(file), false) => read_input(file, "--file")?,
        (None, true) => read_input("-", "stdin")?,
        (Some(_), true) => {
            return Err(Stop::new(
                Exit::Usage,
                "give the list as --file FILE or -, not both",
            ));
        }
        (None, false) => {
            return Err(Stop::new(
                Exit::Usage,
                "give the list as --file FILE, or - to read stdin",
            ));
        }
    };
    let (entries, wanted) = parse_apply(&text)?;
    let pr = prs.first().map(|s| s.as_str()).or(args.pr.as_deref());
    write_threads(
        args,
        pr,
        &entries,
        &wanted,
        a.ignore_resolved,
        a.dry_run,
        out,
    )
}

/// An `apply` list: entries for threads (numeric IDs), read as before, and
/// for Entire findings (`entire:ID`).
fn parse_apply(text: &str) -> Result<(Vec<Entry>, Vec<Wanted>), Stop> {
    let usage = |m: String| Stop::new(Exit::Usage, m);
    let list: Vec<serde_json::Value> =
        serde_json::from_str(text).map_err(|e| usage(format!("apply file: {e}")))?;
    let (findings, threads): (Vec<_>, Vec<_>) = list.into_iter().partition(|e| {
        e["id"]
            .as_str()
            .is_some_and(|id| entire::handle(id).is_some())
    });
    let mut wanted: Vec<Wanted> = Vec::new();
    for e in &findings {
        let id = e["id"].as_str().unwrap_or_default();
        let flag = |k: &str| e[k].as_bool().unwrap_or(false);
        let change = match (flag("dismiss"), flag("resolve"), flag("unresolve")) {
            (true, _, false) => Change::Dismiss,
            (false, true, false) => Change::Resolve,
            (false, false, true) => Change::Reopen,
            (false, false, false) => {
                return Err(usage(format!(
                    "apply file: {id} neither resolves, dismisses nor unresolves"
                )));
            }
            _ => {
                return Err(usage(format!(
                    "apply file: {id} asks for two changes at once"
                )));
            }
        };
        let reply = match &e["reply"] {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
            _ => return Err(usage(format!("apply file: the reply to {id} is empty"))),
        };
        let w = Wanted::new(id, change, reply)?;
        if wanted.iter().any(|o| o.prefix == w.prefix) {
            return Err(usage(format!("apply file: {id} is listed twice")));
        }
        wanted.push(w);
    }
    let entries = if threads.is_empty() && !findings.is_empty() {
        Vec::new()
    } else {
        let text = serde_json::to_string(&threads).unwrap_or_default();
        write::parse_apply(&text).map_err(usage)?
    };
    Ok((entries, wanted))
}

fn id(s: &str) -> Result<u64, Stop> {
    write::parse_id(s).ok_or_else(|| Stop::new(Exit::Usage, format!("`{s}` is not a thread ID")))
}

/// What a resolve or unresolve names: threads, findings and the PR.
struct Named<'a> {
    ids: Vec<u64>,
    handles: Vec<String>,
    pr: Option<&'a str>,
}

/// Splits arguments into thread IDs (bare numbers), Entire finding handles
/// and at most one PR.
fn ids_and_pr(args: &[String]) -> Result<Named<'_>, Stop> {
    let (handles, args): (Vec<&String>, Vec<&String>) =
        args.iter().partition(|a| entire::handle(a).is_some());
    let (ids, prs): (Vec<&String>, Vec<&String>) =
        args.into_iter().partition(|a| write::parse_id(a).is_some());
    if prs.len() > 1 {
        return Err(Stop::new(
            Exit::Usage,
            format!(
                "expected thread IDs and at most one PR, got {}",
                prs.iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    if ids.is_empty() && handles.is_empty() {
        return Err(Stop::new(Exit::Usage, "name at least one thread ID"));
    }
    let ids = ids.iter().map(|s| id(s)).collect::<Result<Vec<_>, _>>()?;
    Ok(Named {
        ids,
        handles: handles.into_iter().cloned().collect(),
        pr: prs.first().map(|s| s.as_str()),
    })
}

fn read_input(file: &str, what: &str) -> Result<String, Stop> {
    let read = if file == "-" {
        if std::io::stdin().is_terminal() {
            return Err(Stop::new(
                Exit::Usage,
                format!("{what} is a terminal; pipe the input in"),
            ));
        }
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map(|_| text)
    } else {
        std::fs::read_to_string(file)
    };
    read.map_err(|e| {
        let exit = if e.kind() == std::io::ErrorKind::NotFound {
            Exit::NotFound
        } else {
            Exit::Usage
        };
        Stop::new(exit, format!("{what} {file}: {e}"))
    })
}

#[derive(Serialize)]
struct Written {
    repo: String,
    /// Absent when only Entire findings were changed on a branch's trail,
    /// which needs nothing from GitHub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pr: Option<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    results: Vec<Outcome>,
    /// The Entire findings changed, apart from the threads.
    #[serde(skip_serializing_if = "Option::is_none")]
    entire: Option<entire::Written>,
}

fn write_threads(
    args: &ThreadsArgs,
    pr: Option<&str>,
    entries: &[Entry],
    wanted: &[Wanted],
    ignore_resolved: bool,
    dry_run: bool,
    out: &Output,
) -> Result<Exit, Stop> {
    let target = target(pr, args.repo.as_deref())?;
    let config = config()?;
    // Refused before anything is posted when findings are named and
    // `entire` cannot change them.
    let entire = if wanted.is_empty() {
        None
    } else {
        Some(entire::require(entire::Probe::start())?)
    };
    let uses_sha = entries
        .iter()
        .filter_map(|e| e.reply.as_deref())
        .chain(wanted.iter().filter_map(|w| w.message.as_deref()))
        .any(|r| r.contains("{sha}"));
    let sha = if uses_sha { git::short_head() } else { None };
    let footer = config.reply_footer();
    // A branch's findings need nothing from the forge.
    let mut host = match (&target, entries.is_empty() && !uses_sha) {
        (Target::Branch(..), true) => None,
        _ => Some(connect(&target)?),
    };
    let pr = match host.as_mut() {
        Some(host) => Some(fetch(host.as_mut(), &target, false, &config.bots())?),
        None => None,
    };
    let (repo, branch) = match (&pr, &target) {
        (Some(pr), _) => (pr.repo.clone(), pr.head_ref.clone()),
        (None, Target::Branch(repo, branch)) => (repo.clone(), branch.clone()),
        (None, Target::Number(..)) => unreachable!("a PR number is always fetched"),
    };
    let mut results = Vec::new();
    if let Some(pr) = &pr
        && !entries.is_empty()
    {
        let options = Options {
            footer: footer.clone(),
            sha: sha.clone(),
            ignore_resolved,
        };
        results =
            write::plan(entries, pr, &options).map_err(|e| Stop(e.exit, e.message, e.hint))?;
        if results.iter().any(Outcome::changes_resolution)
            && let Some(reason) = host.as_mut().and_then(|h| h.lacks(Feature::Resolve))
        {
            return Err(Stop(
                Exit::Precondition,
                format!("{reason}; nothing was posted"),
                Some("reply without --resolve, or resolve it on the web".into()),
            ));
        }
    }
    let finding_sha = wanted
        .iter()
        .any(|w| w.message.as_deref().is_some_and(|m| m.contains("{sha}")));
    if let (Some(pr), Some(host)) = (&pr, host.as_mut())
        && (results.iter().any(Outcome::posts_sha) || finding_sha)
    {
        check_pushed(host.as_mut(), pr)?;
    }
    let wanted: Vec<Wanted> = wanted
        .iter()
        .map(|w| {
            let mut w = w.clone();
            w.message = w.message.map(|m| {
                let m = match &sha {
                    Some(sha) => m.replace("{sha}", sha),
                    None => m,
                };
                match &footer {
                    Some(footer) => format!("{m}\n\n{footer}"),
                    None => m,
                }
            });
            w
        })
        .collect();
    // Every handle is matched before anything is posted, threads included.
    let plan = match &entire {
        Some(entire) => Some(entire::plan(entire, &repo, &branch, &wanted)?),
        None => None,
    };
    let mut ok = match (&pr, host.as_mut()) {
        _ if dry_run => {
            write::dry_run(&mut results);
            true
        }
        (Some(pr), Some(host)) => write::execute(host.as_mut(), pr, &mut results),
        _ => true,
    };
    let mut changed = None;
    if let (Some(entire), Some(plan)) = (&entire, &plan)
        && ok
    {
        let (written, done) = entire::write(entire, &repo, plan, dry_run);
        ok = done;
        changed = Some(written);
    }
    let written = Written {
        repo: repo.to_string(),
        pr: pr.as_ref().map(|p| p.number),
        dry_run,
        results,
        entire: changed,
    };
    out.result(&written, || render_written(&written));
    Ok(if ok { Exit::Ok } else { Exit::Remote })
}

/// Refuses unless HEAD is on the PR's branch on the forge, so `{sha}` never
/// names a commit reviewers cannot see.
fn check_pushed(host: &mut dyn Host, pr: &PrThreads) -> Result<(), Stop> {
    let short = |s: &str| s[..s.len().min(7)].to_string();
    let head = git::head().ok_or_else(|| {
        Stop::new(
            Exit::Precondition,
            "a reply uses {sha}, but there is no git commit here; nothing was posted",
        )
    })?;
    let on_branch = if head == pr.head_oid {
        true
    } else if let Some(known) = git::is_ancestor(&head, &pr.head_oid) {
        known
    } else {
        // The PR's head is not in this clone: ask the forge.
        host.on_branch(pr, &head)
            .map_err(|e| Stop::new(Exit::Remote, e))?
    };
    if on_branch {
        return Ok(());
    }
    let repo = pr.head_repo.as_ref().unwrap_or(&pr.repo);
    Err(Stop(
        Exit::Precondition,
        format!(
            "HEAD {} is not on {repo}:{} (its head is {}), so {{sha}} would name a commit reviewers cannot see; nothing was posted",
            short(&head),
            pr.head_ref,
            short(&pr.head_oid)
        ),
        Some("push HEAD to the PR's branch first".into()),
    ))
}

fn render_written(w: &Written) -> String {
    let mut s = String::new();
    let mut failed = false;
    for o in &w.results {
        let _ = write!(s, "[{}] ", o.id);
        if let Some(reason) = &o.skipped {
            let _ = writeln!(s, "skipped: {reason}");
            continue;
        }
        let mut done = Vec::new();
        if o.already_replied {
            done.push("reply already posted");
        }
        let words = if w.dry_run {
            [
                (o.replied, "would reply"),
                (o.resolved, "would resolve"),
                (o.unresolved, "would unresolve"),
            ]
        } else {
            [
                (o.replied, "replied"),
                (o.resolved, "resolved"),
                (o.unresolved, "unresolved"),
            ]
        };
        done.extend(words.iter().filter(|(on, _)| *on).map(|(_, w)| *w));
        if o.already_resolved {
            done.push("thread already resolved");
        }
        s.push_str(&done.join(", "));
        if let Some(e) = &o.failed {
            failed = true;
            let _ = write!(s, "{}failed: {e}", if done.is_empty() { "" } else { ", " });
        }
        if let Some(url) = &o.url {
            let _ = write!(s, "  {url}");
        }
        s.push('\n');
        if let Some(body) = &o.body {
            for line in body.lines() {
                let _ = writeln!(s, "{}", format!("  | {line}").trim_end());
            }
        }
    }
    if let Some(e) = &w.entire {
        entire::render_written(&mut s, e, w.dry_run);
        failed |= e.results.iter().any(entire::Outcome::failed);
    }
    if failed {
        s.push_str("stopped at a failure; re-running skips what was already posted and resolved\n");
    }
    s
}
