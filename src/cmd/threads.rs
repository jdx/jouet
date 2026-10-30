//! `jouet pr threads`: listing review threads, and replying to and resolving
//! them.

use super::Output;
use crate::config::Config;
use crate::exit::Exit;
use crate::github::Client;
use crate::pr::target::{self, Arg, Repo, Target};
use crate::pr::threads::condense::{self, condense};
use crate::pr::threads::write::{self, Entry, Options, Outcome};
use crate::pr::threads::{self, Context, Counts, Filter, Kind, Lookup, PrThreads, Status, Thread};
use crate::{ApplyArgs, ReplyArgs, ResolveArgs, ThreadsArgs, ThreadsCmd, UnresolveArgs, git, time};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::{IsTerminal, Read};

/// Lines a condensed comment keeps.
const MAX_LINES: usize = 30;

/// A failure that stops a command: its exit code, message and hint.
struct Stop(Exit, String, Option<String>);

impl Stop {
    fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self(exit, message.into(), None)
    }
}

pub fn run(args: ThreadsArgs, out: &Output) -> Exit {
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
        Some(r) => Some(
            Repo::parse(r).ok_or_else(|| usage(format!("--repo takes OWNER/NAME, not `{r}`")))?,
        ),
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

fn fetch(
    client: &mut Client,
    target: &Target,
    extra: bool,
    bots: &[String],
) -> Result<PrThreads, Stop> {
    threads::fetch(client, target, extra, bots)
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
    #[serde(skip)]
    since_label: Option<&'a str>,
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
    let mut client = Client::from_env();
    let bots = config.bots();
    let pr = fetch(&mut client, &target, args.include_reviews, &bots)?;
    let since = match args.since.as_deref().and_then(|s| s.strip_prefix('@')) {
        Some(sha) => Some(push_time(&mut client, &pr, sha)?),
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
        add_context(&mut client, &pr, &mut shown, n);
    }
    let unresolved = shown.iter().any(|t| t.kind == Kind::Thread && !t.resolved);
    let listing = Listing {
        repo: pr.repo.to_string(),
        pr: pr.number,
        counts: threads::counts(&pr),
        since: since.map(time::format_utc),
        threads: shown,
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

fn push_time(client: &mut Client, pr: &PrThreads, sha: &str) -> Result<i64, Stop> {
    let pushed = threads::pushed_at(client, pr, sha).map_err(|e| match e {
        Lookup::NotFound(m) => Stop::new(Exit::NotFound, format!("--since @{sha}: {m}")),
        Lookup::Usage(m) => Stop::new(Exit::Usage, format!("--since @{sha}: {m}")),
        Lookup::Remote(m) => Stop::new(Exit::Remote, m),
    })?;
    if pushed.estimated {
        eprintln!(
            "jouet: found no push of {} in GitHub's activity log; using its commit time, which can only be earlier",
            &pushed.oid[..pushed.oid.len().min(7)]
        );
    }
    Ok(pushed.at)
}

/// Adds lines of the file around each current thread's line: from the
/// working tree when it is at the PR's head and the file is unchanged, else
/// from GitHub. A file that cannot be read gets no context.
fn add_context(client: &mut Client, pr: &PrThreads, shown: &mut [Thread], n: u32) {
    let local = git::toplevel().filter(|_| git::head().as_deref() == Some(pr.head_oid.as_str()));
    let mut paths: Vec<String> = shown
        .iter()
        .filter(|t| t.line.is_some())
        .filter_map(|t| t.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    let mut files: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut remote = Vec::new();
    for path in paths {
        match local.as_ref().filter(|top| git::unchanged(top, &path)) {
            Some(top) => {
                if let Ok(text) = std::fs::read_to_string(top.join(&path)) {
                    files.insert(path, text);
                }
            }
            None => remote.push(path),
        }
    }
    if !remote.is_empty() {
        match remote_files(client, pr, &remote) {
            Ok(fetched) => files.extend(fetched),
            Err(e) => eprintln!("jouet: could not fetch files for --context ({e})"),
        }
    }
    for t in shown {
        let (Some(path), Some(line)) = (&t.path, t.line) else {
            continue;
        };
        if let Some(text) = files.get(path) {
            t.context = context(text, t.start_line.unwrap_or(line), line, n);
        }
    }
}

/// Reads files at the PR's head, all in one query.
fn remote_files(
    client: &mut Client,
    pr: &PrThreads,
    paths: &[String],
) -> Result<Vec<(String, String)>, String> {
    let mut q = String::from("query($o: String!, $r: String!");
    let mut body = String::new();
    let mut vars = serde_json::json!({ "o": pr.repo.owner, "r": pr.repo.name });
    for (i, path) in paths.iter().enumerate() {
        let _ = write!(q, ", $e{i}: String!");
        let _ = write!(
            body,
            " f{i}: object(expression: $e{i}) {{ ... on Blob {{ text }} }}"
        );
        vars[format!("e{i}")] = format!("{}:{path}", pr.head_oid).into();
    }
    let _ = write!(q, ") {{ repository(owner: $o, name: $r) {{{body} }} }}");
    let data = client.graphql(&q, &vars)?;
    Ok(paths
        .iter()
        .enumerate()
        .filter_map(|(i, path)| {
            let text = data["repository"][format!("f{i}")]["text"].as_str()?;
            Some((path.clone(), text.to_string()))
        })
        .collect())
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
    for t in &l.threads {
        s.push('\n');
        render_thread(&mut s, t, now);
    }
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
    for (on, word) in [(t.outdated, "outdated"), (t.resolved, "resolved")] {
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
    let id = id(&a.id)?;
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
    let entries = vec![Entry {
        id,
        reply: Some(body),
        resolve: a.resolve,
        unresolve: false,
    }];
    let pr = a.pr.as_deref().or(args.pr.as_deref());
    write_threads(args, pr, &entries, a.ignore_resolved, false, a.dry_run, out)
}

fn resolve(args: &ThreadsArgs, a: &ResolveArgs, out: &Output) -> Result<Exit, Stop> {
    let (ids, pr) = ids_and_pr(&a.args)?;
    if a.reply.as_deref().is_some_and(|r| r.trim().is_empty()) {
        return Err(Stop::new(
            Exit::Usage,
            "--reply is empty; nothing was posted",
        ));
    }
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
    write_threads(args, pr, &entries, a.ignore_resolved, false, a.dry_run, out)
}

fn unresolve(args: &ThreadsArgs, a: &UnresolveArgs, out: &Output) -> Result<Exit, Stop> {
    let (ids, pr) = ids_and_pr(&a.args)?;
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
    write_threads(args, pr, &entries, false, false, a.dry_run, out)
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
    let entries = write::parse_apply(&text).map_err(|e| Stop::new(Exit::Usage, e))?;
    let pr = prs.first().map(|s| s.as_str()).or(args.pr.as_deref());
    write_threads(args, pr, &entries, a.ignore_resolved, true, a.dry_run, out)
}

fn id(s: &str) -> Result<u64, Stop> {
    write::parse_id(s).ok_or_else(|| Stop::new(Exit::Usage, format!("`{s}` is not a thread ID")))
}

/// Splits arguments into thread IDs (bare numbers) and at most one PR.
fn ids_and_pr(args: &[String]) -> Result<(Vec<u64>, Option<&str>), Stop> {
    let (ids, prs): (Vec<&String>, Vec<&String>) =
        args.iter().partition(|a| write::parse_id(a).is_some());
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
    if ids.is_empty() {
        return Err(Stop::new(Exit::Usage, "name at least one thread ID"));
    }
    let ids = ids.iter().map(|s| id(s)).collect::<Result<Vec<_>, _>>()?;
    Ok((ids, prs.first().map(|s| s.as_str())))
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
    pr: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    results: Vec<Outcome>,
}

fn write_threads(
    args: &ThreadsArgs,
    pr: Option<&str>,
    entries: &[Entry],
    ignore_resolved: bool,
    retry_safe: bool,
    dry_run: bool,
    out: &Output,
) -> Result<Exit, Stop> {
    let target = target(pr, args.repo.as_deref())?;
    let config = config()?;
    let mut client = Client::from_env();
    let pr = fetch(&mut client, &target, false, &config.bots())?;
    let uses_sha = entries
        .iter()
        .any(|e| e.reply.as_deref().is_some_and(|r| r.contains("{sha}")));
    let options = Options {
        footer: config.reply_footer(),
        sha: if uses_sha { git::short_head() } else { None },
        ignore_resolved,
        retry_safe,
    };
    let mut results =
        write::plan(entries, &pr, &options).map_err(|e| Stop(e.exit, e.message, e.hint))?;
    if results.iter().any(Outcome::posts_sha) {
        check_pushed(&mut client, &pr)?;
    }
    let ok = if dry_run {
        write::dry_run(&mut results);
        true
    } else {
        write::execute(&mut client, &mut results)
    };
    let written = Written {
        repo: pr.repo.to_string(),
        pr: pr.number,
        dry_run,
        results,
    };
    out.result(&written, || render_written(&written));
    Ok(if ok { Exit::Ok } else { Exit::Remote })
}

/// Refuses unless HEAD is on the PR's branch on GitHub, so `{sha}` never
/// names a commit reviewers cannot see.
fn check_pushed(client: &mut Client, pr: &PrThreads) -> Result<(), Stop> {
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
        // The PR's head is not in this clone: ask GitHub.
        let repo = pr.head_repo.as_ref().unwrap_or(&pr.repo);
        let path = format!("/repos/{repo}/compare/{head}...{}", pr.head_oid);
        let compare = client
            .rest_get(&path)
            .map_err(|e| Stop::new(Exit::Remote, e))?;
        compare.is_some_and(|c| matches!(c["status"].as_str(), Some("ahead" | "identical")))
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
    if failed {
        s.push_str("stopped at a failure; re-running skips what was already posted and resolved\n");
    }
    s
}
