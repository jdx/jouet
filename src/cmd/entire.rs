//! The Entire side of `jouet pr threads`: the open findings on the PR's
//! trail, listed after the forge's review threads, and resolving,
//! dismissing and reopening them through the `entire` CLI.
//!
//! A finding is named by a handle, `entire:` and the start of its ID, so it
//! is never taken for a thread's number. Its lifecycle is Entire's own:
//! resolving a finding leaves every GitHub thread as it was, and the other
//! way round.

use super::threads::Stop;
use jouet_github::entire::findings::{self, PREFIX};
use jouet_github::entire::{self, Availability, Change, Entire, Finding};
use jouet_github::pr::target::Repo;
use jouet_github::pr::threads::condense::condense;
use jouet_github::pr::threads::{self, Filter, Status};
use jouet_output::exit::Exit;
use jouet_output::time;
use serde::Serialize;
use std::fmt::Write as _;
use std::thread::JoinHandle;

/// Characters of an ID a handle must give, past `entire:`.
const MIN_HANDLE: usize = 8;
/// The length of a whole ULID, which may be given without `entire:`.
const ULID: usize = 26;

/// Finding out whether `entire` can be asked, started before the forge is
/// read so the two overlap.
pub(super) struct Probe(JoinHandle<Result<Entire, String>>);

impl Probe {
    pub(super) fn start() -> Self {
        Self(std::thread::spawn(Entire::probe))
    }

    fn finish(self) -> Result<Entire, String> {
        self.0
            .join()
            .unwrap_or_else(|_| Err("entire could not be run".into()))
    }
}

/// The findings a listing shows, under `entire` in `--json`.
#[derive(Serialize)]
pub(super) struct Listing {
    #[serde(flatten)]
    availability: Availability,
    #[serde(skip_serializing_if = "Option::is_none")]
    trail: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    findings: Vec<Shown>,
    /// Entire said more findings exist than it let jouet read.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    incomplete: bool,
    /// Why the trail or its findings could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip)]
    status: Status,
}

#[derive(Serialize)]
struct Shown {
    handle: String,
    #[serde(flatten)]
    finding: Finding,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    body_truncated: bool,
    #[serde(skip)]
    created: Option<i64>,
}

/// The findings on the trail of the PR whose head is `branch`, chosen by
/// the listing's filter. Without `full`, bodies are condensed the way
/// comments are.
pub(super) fn list(
    probe: Probe,
    repo: &Repo,
    branch: &str,
    filter: &Filter,
    full: bool,
    max_lines: usize,
) -> Listing {
    let probed = probe.finish();
    let mut listing = Listing {
        availability: Availability::of(&probed),
        trail: None,
        url: None,
        findings: Vec::new(),
        incomplete: false,
        error: None,
        status: filter.status,
    };
    let Ok(entire) = probed else {
        return listing;
    };
    let read = || -> Result<_, String> {
        let Some(trail) = entire.trail(repo, None, branch)? else {
            return Ok(None);
        };
        let all = filter.status != Status::Unresolved;
        Ok(Some((
            trail.clone(),
            entire.findings(repo, trail.number, all)?,
        )))
    };
    let (trail, found) = match read() {
        Ok(Some(found)) => found,
        Ok(None) => return listing,
        Err(e) => {
            eprintln!("jouet: entire could not list the findings of {repo}:{branch}: {e}");
            listing.error = Some(e);
            return listing;
        }
    };
    listing.trail = Some(trail.number);
    listing.url = Some(trail.url).filter(|u| !u.is_empty());
    listing.incomplete = found.incomplete;
    let kept: Vec<Finding> = found
        .findings
        .into_iter()
        .filter(|f| match filter.status {
            Status::Unresolved => f.is_open(),
            Status::Resolved => !f.is_open(),
            Status::All => true,
        })
        .filter(|f| threads::keeps_finding(filter, f))
        .collect();
    let ids: Vec<&str> = kept.iter().map(|f| f.id.as_str()).collect();
    let handles = findings::handles(&ids);
    listing.findings = kept
        .into_iter()
        .zip(handles)
        .map(|(mut finding, handle)| {
            let mut body_truncated = false;
            if !full {
                let c = condense(&finding.body, max_lines);
                finding.body = c.body;
                body_truncated = c.truncated;
            }
            Shown {
                created: time::parse_utc(&finding.created_at),
                handle,
                finding,
                body_truncated,
            }
        })
        .collect();
    listing
}

/// The findings, after the threads: a heading naming the trail and saying
/// the two are separate, then one entry per finding.
pub(super) fn render(s: &mut String, l: &Listing, now: i64) {
    let Some(trail) = l.trail else {
        return;
    };
    let n = l.findings.len();
    let what = match l.status {
        Status::Unresolved => "open finding",
        Status::Resolved => "resolved or dismissed finding",
        Status::All => "finding",
    };
    let _ = write!(
        s,
        "\nEntire trail #{trail}: {} {what}{}",
        if n == 0 { "no".into() } else { n.to_string() },
        if n == 1 { "" } else { "s" }
    );
    if l.incomplete {
        s.push_str(" (Entire listed more than it let jouet read)");
    }
    s.push('\n');
    if n == 0 {
        return;
    }
    s.push_str(
        "These are Entire's, not GitHub threads: resolving one leaves the other as it was.\n",
    );
    for shown in &l.findings {
        let f = &shown.finding;
        let _ = write!(s, "\n[{}] {}", shown.handle, f.location());
        if let Some(severity) = &f.severity {
            let _ = write!(s, "  {severity}");
        }
        if !f.author.is_empty() {
            let _ = write!(s, "  {}", f.author);
        }
        if let Some(created) = shown.created {
            let _ = write!(s, "  {} ago", time::age(now - created));
        }
        if f.stale {
            s.push_str("  stale");
        }
        if !f.is_open() {
            let _ = write!(s, "  {}", f.status);
        }
        s.push('\n');
        for line in f.body.lines() {
            if line.is_empty() {
                s.push('\n');
            } else {
                let _ = writeln!(s, "  {line}");
            }
        }
        if let Some(reason) = &f.status_reason {
            let _ = writeln!(s, "  {}: {reason}", f.status);
        }
    }
    if l.findings.iter().any(|f| f.finding.is_open()) {
        s.push_str(
            "\nResolve a finding with `jouet pr threads resolve entire:ID --reply REASON`, or \
             add --dismiss to dismiss it, which needs a reason.\n",
        );
    }
}

/// The ID start a handle names: `entire:` and at least eight characters of
/// a ULID, or a whole ULID on its own.
pub(super) fn handle(s: &str) -> Option<String> {
    let (rest, prefixed) = match s.get(..PREFIX.len()) {
        Some(p) if p.eq_ignore_ascii_case(PREFIX) => (&s[PREFIX.len()..], true),
        _ => (s, false),
    };
    let alnum = !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric());
    let long_enough = if prefixed {
        rest.len() >= MIN_HANDLE
    } else {
        rest.len() == ULID && rest.bytes().any(|b| b.is_ascii_alphabetic())
    };
    (alnum && long_enough).then(|| rest.to_ascii_uppercase())
}

/// One change asked of a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Wanted {
    /// As given, for the output.
    pub(super) given: String,
    /// The ID start it names.
    pub(super) prefix: String,
    pub(super) change: Change,
    pub(super) message: Option<String>,
}

impl Wanted {
    /// A change, refused (exit 2) when it would dismiss without a reason.
    pub(super) fn new(given: &str, change: Change, message: Option<String>) -> Result<Self, Stop> {
        let prefix = handle(given)
            .ok_or_else(|| Stop::new(Exit::Usage, format!("`{given}` is not a finding handle")))?;
        let message = message.filter(|m| !m.trim().is_empty());
        if change == Change::Dismiss && message.is_none() {
            return Err(Stop(
                Exit::Usage,
                format!("dismissing {given} needs a reason; nothing was changed"),
                Some("give it with --reply (resolve), --body (reply) or \"reply\" (apply)".into()),
            ));
        }
        Ok(Self {
            given: given.to_string(),
            prefix,
            change,
            message,
        })
    }
}

/// What happened to one finding, or in a dry run what would.
#[derive(Debug, Serialize)]
pub(super) struct Outcome {
    /// The handle as given.
    id: String,
    /// The whole ID it named.
    finding: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    resolved: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dismissed: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    reopened: bool,
    /// It was already closed (`resolved` or `dismissed`) or, to reopen,
    /// `open`, and was left alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    already: Option<String>,
    /// In a dry run, the `entire` command that would run.
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed: Option<String>,
    #[serde(skip)]
    change: Change,
}

impl Outcome {
    pub(super) fn failed(&self) -> bool {
        self.failed.is_some()
    }
}

/// The changes made on a trail.
#[derive(Debug, Serialize)]
pub(super) struct Written {
    pub(super) trail: u64,
    pub(super) results: Vec<Outcome>,
}

/// `entire`, or a refusal (exit 4) saying why it cannot change findings.
pub(super) fn require(probe: Probe) -> Result<Entire, Stop> {
    probe.finish().map_err(|reason| {
        Stop(
            Exit::Precondition,
            format!("{reason}, so Entire findings cannot be changed; nothing was changed"),
            Some(
                "findings are changed through the entire CLI: install it and run `entire login`"
                    .into(),
            ),
        )
    })
}

/// The changes asked of findings on one trail, each handle matched to one
/// finding.
pub(super) struct Plan {
    trail: u64,
    steps: Vec<(Wanted, Finding)>,
}

/// Matches every handle to one finding on the trail of `branch`, changing
/// nothing, so a bad handle stops a call before anything is posted
/// anywhere, GitHub threads included.
pub(super) fn plan(
    entire: &Entire,
    repo: &Repo,
    branch: &str,
    wanted: &[Wanted],
) -> Result<Plan, Stop> {
    let remote = |e: String| Stop::new(Exit::Remote, e);
    let trail = entire
        .trail(repo, None, branch)
        .map_err(remote)?
        .ok_or_else(|| {
            Stop::new(
                Exit::NotFound,
                format!("no Entire trail tracks {repo}:{branch}; nothing was changed"),
            )
        })?;
    let all = entire.findings(repo, trail.number, true).map_err(remote)?;
    let mut planned = Vec::new();
    for w in wanted {
        let matching: Vec<&Finding> = all
            .findings
            .iter()
            .filter(|f| f.id.to_ascii_uppercase().starts_with(&w.prefix))
            .collect();
        let finding = match matching.as_slice() {
            [one] => *one,
            [] => {
                return Err(Stop::new(
                    Exit::NotFound,
                    format!(
                        "no finding {} on Entire trail #{}; nothing was changed",
                        w.given, trail.number
                    ),
                ));
            }
            many => {
                let ids: Vec<&str> = many.iter().map(|f| f.id.as_str()).collect();
                return Err(Stop::new(
                    Exit::Usage,
                    format!(
                        "{} names {} findings ({}); give more of the ID",
                        w.given,
                        many.len(),
                        ids.join(", ")
                    ),
                ));
            }
        };
        planned.push((w.clone(), finding.clone()));
    }
    Ok(Plan {
        trail: trail.number,
        steps: planned,
    })
}

/// Makes the planned changes in order, stopping at the first that fails. A
/// finding already closed (or, to reopen, open) is left alone.
pub(super) fn write(entire: &Entire, repo: &Repo, plan: &Plan, dry_run: bool) -> (Written, bool) {
    let mut results = Vec::new();
    let mut ok = true;
    for (w, finding) in &plan.steps {
        // Resolved or dismissed, a finding no longer blocks: either is left
        // alone by a resolve or a dismiss, as a resolved thread is.
        let already = match w.change {
            Change::Reopen => finding.is_open(),
            Change::Resolve | Change::Dismiss => !finding.is_open(),
        };
        let mut outcome = Outcome {
            id: w.given.clone(),
            finding: finding.id.clone(),
            resolved: false,
            dismissed: false,
            reopened: false,
            already: None,
            command: None,
            failed: None,
            change: w.change,
        };
        if already {
            outcome.already = Some(finding.status.clone());
        } else if !ok {
            outcome.failed = Some("not tried after an earlier failure".into());
        } else {
            if dry_run {
                let args = entire::change_args(
                    repo,
                    plan.trail,
                    &finding.id,
                    w.change,
                    w.message.as_deref(),
                );
                let words: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
                outcome.command = Some(format!("entire {}", words.join(" ")));
            } else if let Err(e) = entire.change(
                repo,
                plan.trail,
                &finding.id,
                w.change,
                w.message.as_deref(),
            ) {
                outcome.failed = Some(e);
                ok = false;
            }
            if outcome.failed.is_none() {
                match w.change {
                    Change::Resolve => outcome.resolved = true,
                    Change::Dismiss => outcome.dismissed = true,
                    Change::Reopen => outcome.reopened = true,
                }
            }
        }
        results.push(outcome);
    }
    (
        Written {
            trail: plan.trail,
            results,
        },
        ok,
    )
}

/// One line per finding changed, and a line saying GitHub threads were not
/// touched by it.
pub(super) fn render_written(s: &mut String, w: &Written, dry_run: bool) {
    for o in &w.results {
        let _ = write!(s, "[{}] ", o.id);
        if let Some(state) = &o.already {
            let _ = writeln!(s, "already {state} on Entire trail #{}", w.trail);
            continue;
        }
        let done = [
            (o.resolved, Change::Resolve),
            (o.dismissed, Change::Dismiss),
            (o.reopened, Change::Reopen),
        ]
        .into_iter()
        .find(|(on, _)| *on)
        .map(|(_, c)| c);
        match (done, &o.failed) {
            (Some(c), _) if dry_run => {
                let _ = write!(s, "would {} on Entire trail #{}", c.verb(), w.trail);
            }
            (Some(c), _) => {
                let _ = write!(s, "{} on Entire trail #{}", c.done(), w.trail);
            }
            (None, Some(e)) => {
                let _ = write!(s, "{} failed: {e}", o.change.verb());
            }
            (None, None) => {}
        }
        s.push('\n');
        if let Some(command) = &o.command {
            let _ = writeln!(s, "  | {command}");
        }
    }
    s.push_str(
        "Entire findings are separate from GitHub threads: no thread was changed by these.\n",
    );
}

/// `s` as one shell word: as it is when that is safe, else single-quoted.
fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:@%+=,".contains(&b));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_need_the_prefix_or_a_whole_ulid() {
        assert_eq!(handle("entire:01m4c997ea").as_deref(), Some("01M4C997EA"));
        assert_eq!(
            handle("01M4C997DZH5KQRP31NW93NQHR").as_deref(),
            Some("01M4C997DZH5KQRP31NW93NQHR")
        );
        assert_eq!(handle("entire:01M4"), None, "too short to name one");
        assert_eq!(handle("01M4C997EA"), None, "a bare prefix needs entire:");
        assert_eq!(
            handle("12345678901234567890123456"),
            None,
            "a number is a thread"
        );
        assert_eq!(handle("2914571234"), None);
    }

    #[test]
    fn dismissing_needs_a_reason() {
        let Err(Stop(exit, message, _)) = Wanted::new("entire:01M4C997EA", Change::Dismiss, None)
        else {
            panic!("a dismissal without a reason was accepted");
        };
        assert_eq!(exit, Exit::Usage);
        assert!(message.contains("needs a reason"), "{message}");
        assert!(Wanted::new("entire:01M4C997EA", Change::Resolve, None).is_ok());
    }
}
