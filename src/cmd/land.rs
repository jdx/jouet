//! `jouet pr land`, after steipete's `/landpr`: refuse unless `pr next` says
//! the PR is ready to merge, optionally update it with its base, wait for
//! the new head's checks, merge through the API, and confirm the merge.
//!
//! It is the one jouet command that can ask a question, and only a person
//! at a terminal is asked: stdin and stderr are terminals and no coding
//! agent is detected. Anyone else passes `--yes` (or `--dry-run`), or gets a
//! usage error before anything is sent to GitHub.

use super::Output;
use super::pr;
use crate::LandArgs;
use jouet_github::Client;
use jouet_github::pr::land::{self, Method, Refused};
use jouet_github::pr::next::{self, Action, Next};
use jouet_github::pr::status::{Lookup, PrStatus};
use jouet_github::pr::target::{Repo, Target};
use jouet_output::display;
use jouet_output::exit::Exit;
use jouet_output::time::{elapsed, parse_duration};
use jouet_wait::checks::Selection;
use jouet_wait::ci::{self, CiWait};
use jouet_wait::remote::{self, Watch};
use jouet_wait::{Reporter, Timing};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);
const APPEAR_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const LOG_LINES: usize = 60;
/// How long GitHub may take to push the merge commit of an update.
const HEAD_MOVE_TIMEOUT: Duration = Duration::from_secs(60);
/// How many times a merge is looked for after GitHub accepted it.
const CONFIRM_TRIES: u32 = 5;

/// The result document.
#[derive(Debug, Serialize)]
struct Landed {
    repo: String,
    pr: u64,
    url: String,
    /// `merged`, `dry_run`, `ci_failed`, `conflict` or `timeout`.
    result: &'static str,
    method: &'static str,
    /// The head that was (or would be) merged.
    head_sha: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    updated: bool,
    plan: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    merge_commit: Option<String>,
    /// The checks waited for after `--update`.
    #[serde(skip_serializing_if = "Option::is_none")]
    ci: Option<ci::Doc>,
    /// The checks' text result, for text output.
    #[serde(skip)]
    ci_text: String,
}

/// An error that stops the landing.
struct Stop {
    exit: Exit,
    message: String,
    hint: Option<String>,
}

impl Stop {
    fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

struct Options {
    method: Method,
    update: bool,
    dry_run: bool,
    ask: bool,
    timeout: Duration,
    interval: Duration,
    settle: Duration,
}

pub(crate) fn run(args: &LandArgs, out: &Output) -> Exit {
    let options = match options(args) {
        Ok(options) => options,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    if options.ask && (out.json || !display::person_at_terminal()) {
        let message = if out.json {
            "pr land merges the PR and cannot ask with --json, so it needs --yes"
        } else {
            "pr land merges the PR, and with no person at a terminal to confirm it needs --yes"
        };
        return out.error_with_hint(
            Exit::Usage,
            message,
            Some("pass --yes to merge without asking, or --dry-run to see the plan"),
        );
    }
    let target = match super::wait::target(args.pr.as_deref(), args.repo.as_deref()) {
        Ok(target) => target,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    match land(&mut Client::from_env(), &target, &options, out) {
        Ok((landed, exit)) => {
            out.result(&landed, || render(&landed));
            exit
        }
        Err(stop) => out.error_with_hint(stop.exit, &stop.message, stop.hint.as_deref()),
    }
}

fn options(args: &LandArgs) -> Result<Options, String> {
    let method = match args.method.as_deref() {
        None => Method::Squash,
        Some(m) => Method::parse(m)
            .ok_or_else(|| format!("--method must be squash, merge or rebase, not `{m}`"))?,
    };
    let timeout = match args.timeout.as_deref() {
        None => DEFAULT_TIMEOUT,
        Some(d) => parse_duration(d).map_err(|e| format!("--timeout: {e}"))?,
    };
    let interval = match args.interval.as_deref() {
        None => DEFAULT_INTERVAL,
        Some(d) => parse_duration(d).map_err(|e| format!("--interval: {e}"))?,
    };
    if interval.is_zero() {
        return Err("--interval must be longer than 0".into());
    }
    Ok(Options {
        method,
        update: args.update,
        dry_run: args.dry_run,
        ask: !args.yes && !args.dry_run,
        timeout,
        interval,
        settle: pr::settle_arg(args.settle.as_deref())?,
    })
}

fn land(
    client: &mut Client,
    target: &Target,
    options: &Options,
    out: &Output,
) -> Result<(Landed, Exit), Stop> {
    let (mut pr, next) = look(client, target, options.settle)?;
    let name = format!("{}#{}", pr.repo, pr.number);
    let update = gate(&pr, &next, options.update)?;
    let repo = Repo::parse(&pr.repo).expect("GitHub names repositories OWNER/NAME");
    let mut landed = Landed {
        repo: pr.repo.clone(),
        pr: pr.number,
        url: pr.url.clone(),
        result: "dry_run",
        method: options.method.as_str(),
        head_sha: pr.head_sha.clone(),
        updated: false,
        plan: plan(&pr, update, options),
        merge_commit: None,
        ci: None,
        ci_text: String::new(),
    };
    if options.dry_run {
        return Ok((landed, Exit::Ok));
    }
    if options.ask && !confirm(&name, &landed.plan) {
        return Err(Stop::new(
            Exit::Precondition,
            format!("{name} was not merged: not confirmed"),
        ));
    }
    if update {
        eprintln!("jouet: merging {} into {name} on GitHub", pr.base);
        land::update_branch(client, &repo, pr.number, &pr.head_oid)
            .map_err(|r| refused(&name, "update", r))?;
        let head = moved_head(client, &repo, pr.number, &pr.head_oid)?;
        landed.updated = true;
        landed.head_sha = head.chars().take(10).collect();
        let (doc, exit, text) = wait_for_checks(client, &repo, pr.number, &head, options, out)?;
        landed.ci = Some(doc);
        if exit != Exit::Ok {
            landed.result = match exit {
                Exit::Timeout => "timeout",
                Exit::Precondition => "conflict",
                _ => "ci_failed",
            };
            landed.ci_text = text;
            return Ok((landed, exit));
        }
        // The update can dismiss approvals or start new required checks,
        // so the gate is passed again, on the head the checks ran on.
        let (fresh, next) = look(client, target, options.settle)?;
        if fresh.head_oid != head {
            return Err(Stop::new(
                Exit::Precondition,
                format!(
                    "{name}'s head moved to {} while its checks ran; nothing was merged",
                    fresh.head_sha
                ),
            )
            .hint(format!("jouet pr next {name}")));
        }
        gate(&fresh, &next, false)?;
        pr = fresh;
    }
    eprintln!("jouet: {} {name} at {}", verb(options.method), pr.head_sha);
    let reported = match land::merge(client, &repo, pr.number, &pr.head_oid, options.method) {
        Ok(sha) => sha,
        // The merge may have happened before the connection dropped, so
        // GitHub is asked.
        Err(Refused::Failed(f)) => {
            eprintln!(
                "jouet: the merge request failed ({}); checking whether it merged",
                f.message()
            );
            None
        }
        Err(r) => return Err(refused(&name, "merge", r)),
    };
    landed.merge_commit = Some(confirmed(client, &repo, pr.number, reported)?);
    landed.result = "merged";
    Ok((landed, Exit::Ok))
}

/// Fetches the PR as `pr status` does, and what `pr next` says of it.
fn look(client: &mut Client, target: &Target, settle: Duration) -> Result<(PrStatus, Next), Stop> {
    let report = pr::fetch(client, vec![Lookup::Pr(target.clone())], settle)
        .map_err(|e| Stop::new(Exit::Remote, e))?;
    let Some(pr) = report.prs.into_iter().next() else {
        return Err(Stop::new(Exit::NotFound, format!("no PR for {target}")));
    };
    let next = next::next(&pr);
    Ok((pr, next))
}

/// Whether the PR may be landed, and whether it needs updating first.
fn gate(pr: &PrStatus, next: &Next, update: bool) -> Result<bool, Stop> {
    let name = format!("{}#{}", pr.repo, pr.number);
    match next.action {
        Action::ReadyToMerge => return Ok(false),
        Action::BehindBase if update => return Ok(true),
        Action::Merged => {
            return Err(Stop::new(
                Exit::Precondition,
                format!("{name} is already merged"),
            ));
        }
        _ => {}
    }
    let mut stop = Stop::new(
        Exit::Precondition,
        format!(
            "{name} is not ready to merge: {} ({})",
            next.action.as_str(),
            next.reason
        ),
    );
    if next.action == Action::BehindBase {
        stop = stop.hint(format!(
            "pass --update to merge {} into it on GitHub, or run `{}`",
            pr.base,
            next.command.as_deref().unwrap_or_default()
        ));
    } else if let Some(command) = &next.command {
        stop = stop.hint(format!("next: {command}"));
    }
    Err(stop)
}

fn plan(pr: &PrStatus, update: bool, options: &Options) -> Vec<String> {
    let name = format!("{}#{}", pr.repo, pr.number);
    let mut plan = Vec::new();
    let head = if update {
        plan.push(format!(
            "merge {} into {} on GitHub (update-branch, never a force-push)",
            pr.base, pr.head
        ));
        plan.push(format!(
            "wait up to {} for the checks on the new head",
            elapsed(options.timeout.as_secs())
        ));
        "the new head".to_string()
    } else {
        pr.head_sha.clone()
    };
    plan.push(format!(
        "{} {name} at {head} through the API, honouring branch protection",
        verb(options.method)
    ));
    plan.push(format!("confirm {name} is merged"));
    plan
}

fn verb(method: Method) -> &'static str {
    match method {
        Method::Squash => "squash-merge",
        Method::Merge => "merge",
        Method::Rebase => "rebase-merge",
    }
}

/// Shows the plan on stderr and asks; only `y` or `yes` goes ahead.
fn confirm(name: &str, plan: &[String]) -> bool {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "jouet pr land will:");
    for (i, step) in plan.iter().enumerate() {
        let _ = writeln!(stderr, "  {}. {step}", i + 1);
    }
    let _ = write!(stderr, "Merge {name}? [y/N] ");
    let _ = stderr.flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn refused(name: &str, what: &str, r: Refused) -> Stop {
    match r {
        Refused::Rejected(message) => Stop::new(
            Exit::Precondition,
            format!("GitHub refused to {what} {name}: {message}"),
        )
        .hint(format!(
            "nothing was merged; jouet never bypasses branch protection. See `jouet pr next {name}`"
        )),
        Refused::NotFound(message) => Stop::new(Exit::NotFound, message),
        Refused::Failed(f) => Stop::new(Exit::Remote, f.message()),
    }
}

/// Waits for GitHub to push the update's merge commit, and returns the new
/// head.
fn moved_head(client: &mut Client, repo: &Repo, number: u64, old: &str) -> Result<String, Stop> {
    let target = Target::Number(repo.clone(), number);
    let deadline = Instant::now() + HEAD_MOVE_TIMEOUT;
    loop {
        let report = pr::fetch(client, vec![Lookup::Pr(target.clone())], Duration::ZERO)
            .map_err(|e| Stop::new(Exit::Remote, e))?;
        if let Some(pr) = report.prs.first()
            && pr.head_oid != old
        {
            return Ok(pr.head_oid.clone());
        }
        if Instant::now() >= deadline {
            return Err(Stop::new(
                Exit::Remote,
                format!(
                    "GitHub accepted the update of {target} but its head did not move within {}",
                    elapsed(HEAD_MOVE_TIMEOUT.as_secs())
                ),
            ));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `wait ci` on the updated head: its result document, exit code and text.
fn wait_for_checks(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    head: &str,
    options: &Options,
    out: &Output,
) -> Result<(ci::Doc, Exit, String), Stop> {
    let ci_options = ci::Options {
        selection: Selection::default(),
        all: false,
        log_lines: Some(LOG_LINES),
        appear_timeout: APPEAR_TIMEOUT,
    };
    let mut w = CiWait::new(repo.clone(), number, Some(head.to_string()), ci_options);
    // Progress goes to stderr; the result is part of land's own document.
    let mut reporter = Reporter::new(out.mode, false, false, false, w.label());
    let timing = Timing {
        timeout: options.timeout,
        interval: options.interval,
    };
    let doc = remote::wait(&mut w, client, &timing, &mut reporter)
        .map_err(|s| Stop::new(s.exit, s.message))?;
    let exit = w.exit(&doc);
    let text = w.text(&doc, &reporter);
    Ok((doc, exit, text))
}

/// Confirms GitHub reports the PR as merged, and returns its merge commit.
fn confirmed(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    reported: Option<String>,
) -> Result<String, Stop> {
    for attempt in 1..=CONFIRM_TRIES {
        let merged = land::merged(client, repo, number)
            .map_err(|r| refused(&format!("{repo}#{number}"), "read", r))?;
        if merged.merged {
            return merged
                .merge_commit
                .or(reported)
                .ok_or_else(|| Stop::new(Exit::Remote, "GitHub reports no merge commit"));
        }
        if attempt < CONFIRM_TRIES {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Err(Stop::new(
        Exit::Remote,
        format!("GitHub does not report {repo}#{number} as merged"),
    )
    .hint(format!("check with `jouet pr status {repo}#{number}`")))
}

fn render(landed: &Landed) -> String {
    let name = format!("{}#{}", landed.repo, landed.pr);
    let mut s = String::new();
    match landed.result {
        "merged" => {
            let _ = writeln!(
                s,
                "merged {name} ({}) as {}",
                landed.method,
                landed.merge_commit.as_deref().unwrap_or_default()
            );
            let _ = writeln!(s, "  {}", landed.url);
        }
        "dry_run" => {
            let _ = writeln!(s, "jouet pr land would:");
            for (i, step) in landed.plan.iter().enumerate() {
                let _ = writeln!(s, "  {}. {step}", i + 1);
            }
            s.push_str("dry run: nothing was changed\n");
        }
        _ => {
            s.push_str(&landed.ci_text);
            let why = match landed.result {
                "timeout" => "its checks were still running at --timeout",
                "conflict" => "it conflicts with its base after the update",
                _ => "its checks failed after the update",
            };
            let _ = writeln!(s, "not merged: {why}");
        }
    }
    s
}
