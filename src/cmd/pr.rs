//! `jouet pr status`.

use super::Output;
use crate::exit::Exit;
use crate::github::Client;
use crate::pr::status::{self, Fetched, Incomplete, Lookup, Mergeability, PrStatus, State};
use crate::pr::target::{self, Arg, Repo, Target};
use crate::time;
use crate::{PrArgs, PrCmd, StatusArgs};
use serde::Serialize;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

const DEFAULT_SETTLE: Duration = Duration::from_secs(20);
/// Titles longer than this are cut at a word boundary in text output.
const TITLE_WIDTH: usize = 40;

pub fn run(args: PrArgs, out: &Output) -> Exit {
    match args.cmd {
        PrCmd::Status(args) => run_status(&args, out),
    }
}

#[derive(Debug, Serialize)]
struct Report {
    prs: Vec<PrStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    not_found: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    incomplete: Vec<Incomplete>,
}

fn run_status(args: &StatusArgs, out: &Output) -> Exit {
    let settle = match args.settle.as_deref().map(time::parse_duration) {
        None => DEFAULT_SETTLE,
        Some(Ok(d)) => d,
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--settle: {e}")),
    };
    let lookups = match lookups(args) {
        Ok(lookups) => lookups,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let mut client = Client::from_env();
    let mut fetched = match status::fetch(&mut client, lookups) {
        Ok(fetched) => fetched,
        Err(e) => return out.error(Exit::Remote, &e),
    };
    if let Err(e) = settle_mergeable(&mut client, &mut fetched.prs, settle) {
        return out.error(Exit::Remote, &e);
    }
    let now = time::now();
    let report = Report {
        prs: fetched.prs.into_iter().map(|p| p.finish(now)).collect(),
        not_found: fetched.not_found,
        incomplete: fetched.incomplete,
    };
    out.result(&report, || render(&report));
    if !report.not_found.is_empty() {
        Exit::NotFound
    } else if args.fail_on_attention && report.prs.iter().any(|p| !p.attention.is_empty()) {
        Exit::Failed
    } else {
        Exit::Ok
    }
}

/// Turns the arguments and selectors into lookups, in the order given.
fn lookups(args: &StatusArgs) -> Result<Vec<Lookup>, String> {
    let repos = args
        .repo
        .iter()
        .map(|r| Repo::parse(r).ok_or_else(|| format!("--repo takes OWNER/NAME, not `{r}`")))
        .collect::<Result<Vec<_>, _>>()?;
    let git_repo = std::cell::OnceCell::new();
    let git_repo = || git_repo.get_or_init(target::repo_from_git).clone();
    let repo_for = |arg: &str| match repos.as_slice() {
        [repo] => Ok(repo.clone()),
        [] => git_repo().ok_or_else(|| {
            format!("no --repo given and no GitHub remote here, so `{arg}` names no repository")
        }),
        _ => Err(format!(
            "`{arg}` is ambiguous with {} --repo values; write it as OWNER/REPO#N",
            repos.len()
        )),
    };
    let mut lookups = Vec::new();
    for arg in &args.prs {
        let target = match Arg::parse(arg) {
            Arg::Number(n) => Target::Number(repo_for(arg)?, n),
            Arg::Branch(branch) => Target::Branch(repo_for(arg)?, branch),
            Arg::InRepo(repo, n) => Target::Number(repo, n),
        };
        lookups.push(Lookup::Pr(target));
    }
    // Searches cover every --repo, else this checkout's repository, else all
    // of GitHub.
    let scope: Vec<Repo> = if repos.is_empty() {
        git_repo().into_iter().collect()
    } else {
        repos.clone()
    };
    if args.mine {
        lookups.push(search("is:open author:@me", &scope));
    }
    if let Some(query) = &args.search {
        let scoped = ["repo:", "org:", "user:"]
            .iter()
            .any(|q| query.split_whitespace().any(|w| w.starts_with(q)));
        lookups.push(search(query, if scoped { &[] } else { &scope }));
    }
    if lookups.is_empty() {
        let branch = target::current_branch()
            .ok_or("not on a branch: name a PR, or use --mine or --search")?;
        let repo = repo_for(&branch)?;
        lookups.push(Lookup::Pr(Target::Branch(repo, branch)));
    }
    Ok(lookups)
}

fn search(query: &str, repos: &[Repo]) -> Lookup {
    let mut query = format!("is:pr {query}");
    for repo in repos {
        let _ = write!(query, " repo:{repo}");
    }
    Lookup::Search {
        query,
        after: None,
        listed: 0,
    }
}

/// Re-queries open PRs whose mergeable state is still unknown until GitHub
/// settles it or `settle` runs out.
fn settle_mergeable(
    client: &mut Client,
    prs: &mut [Fetched],
    settle: Duration,
) -> Result<(), String> {
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
        let again = status::fetch(client, unknown.into_iter().map(Lookup::Pr).collect())?;
        for fresh in again.prs {
            if let Some(pr) = prs.iter_mut().find(|p| p.key() == fresh.key()) {
                *pr = fresh;
            }
        }
    }
}

/// One line per PR, a line naming any that were not found, then the summary.
fn render(report: &Report) -> String {
    let one_repo = report.prs.windows(2).all(|w| w[0].repo == w[1].repo);
    let name = |p: &PrStatus| {
        if one_repo {
            format!("#{}", p.number)
        } else {
            format!("{}#{}", p.repo, p.number)
        }
    };
    let rows: Vec<[String; 5]> = report
        .prs
        .iter()
        .map(|p| {
            [
                name(p),
                p.state.as_str().to_string(),
                cut_title(&p.title),
                merge_word(p).to_string(),
                ci(p),
            ]
        })
        .collect();
    let width = |col: usize| {
        rows.iter()
            .map(|r| r[col].chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths = [width(0), width(1), width(2), width(3), width(4)];
    let mut s = String::new();
    for (p, row) in report.prs.iter().zip(&rows) {
        let review = match p.review.as_deref() {
            None => "-",
            Some("review_required") => "required",
            Some(r) => r,
        };
        let mut line = format!(
            "{:<w0$} {:<w1$} {:<w2$}  {:<w3$}  {:<w4$}  threads {}  review {review}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            p.unresolved_threads,
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
        );
        if p.auto_merge {
            line.push_str("  auto-merge");
        }
        s.push_str(&line);
        s.push('\n');
    }
    if !report.not_found.is_empty() {
        let _ = writeln!(s, "not found: {}", report.not_found.join(", "));
    }
    for i in &report.incomplete {
        let _ = writeln!(
            s,
            "search `{}` matched {} PRs but GitHub listed only {} (it lists at most 1000)",
            i.query, i.matched, i.listed
        );
    }
    let n = report.prs.len();
    let _ = write!(s, "{n} {}", if n == 1 { "PR" } else { "PRs" });
    let needing: Vec<String> = report
        .prs
        .iter()
        .filter(|p| !p.attention.is_empty())
        .map(|p| {
            let codes: Vec<&str> = p.attention.iter().map(|a| a.code).collect();
            format!("{} ({})", name(p), codes.join(", "))
        })
        .collect();
    match needing.len() {
        0 if n > 0 => s.push_str(", nothing needs attention"),
        0 => {}
        1 => {
            let _ = write!(s, ", 1 needs attention: {}", needing[0]);
        }
        k => {
            let _ = write!(s, ", {k} need attention: {}", needing.join(", "));
        }
    }
    if !report.not_found.is_empty() {
        let _ = write!(s, "; {} not found", report.not_found.len());
    }
    let unlisted: u64 = report.incomplete.iter().map(|i| i.matched - i.listed).sum();
    if unlisted > 0 {
        let _ = write!(
            s,
            "; {unlisted} more matched the search but were not listed"
        );
    }
    s.push('\n');
    s
}

/// Cuts a title to `TITLE_WIDTH` characters at a word boundary, marking the
/// cut with `...`.
fn cut_title(title: &str) -> String {
    if title.chars().count() <= TITLE_WIDTH {
        return title.to_string();
    }
    let head: String = title.chars().take(TITLE_WIDTH).collect();
    let head = match head.rfind(' ') {
        Some(i) if i >= TITLE_WIDTH / 2 => &head[..i],
        _ => &head,
    };
    format!("{}...", head.trim_end())
}

fn merge_word(p: &PrStatus) -> &'static str {
    if !matches!(p.state, State::Open | State::Draft) {
        return "-";
    }
    match p.mergeable {
        Mergeability::Mergeable => "ok",
        Mergeability::Conflicting => "CONFLICT",
        Mergeability::Unknown => "unknown",
    }
}

/// `ci 41/46 fail: lint, unit`: passing out of all checks, then what failed
/// and whether any are still running.
fn ci(p: &PrStatus) -> String {
    let c = &p.checks;
    if c.total() == 0 {
        return "ci none".into();
    }
    let mut s = format!("ci {}/{}", c.pass, c.total());
    if c.fail > 0 {
        let _ = write!(s, " fail: {}", c.failing.join(", "));
    }
    if c.pending > 0 {
        s.push_str(if c.fail > 0 { "; pending" } else { " pending" });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cut_at_a_word() {
        assert_eq!(
            cut_title("docs: describe current trust rules"),
            "docs: describe current trust rules"
        );
        assert_eq!(
            cut_title("fix(vfox): bound header lock downloads by http_download_timeout"),
            "fix(vfox): bound header lock downloads..."
        );
        let long_word = "x".repeat(50);
        assert_eq!(cut_title(&long_word), format!("{}...", "x".repeat(40)));
    }

    #[test]
    fn a_cut_short_search_is_reported() {
        let report = Report {
            prs: Vec::new(),
            not_found: Vec::new(),
            incomplete: vec![Incomplete {
                query: "is:pr is:merged repo:jdx/mise".into(),
                matched: 1234,
                listed: 1000,
            }],
        };
        assert_eq!(
            render(&report),
            "search `is:pr is:merged repo:jdx/mise` matched 1234 PRs but GitHub listed only 1000 \
             (it lists at most 1000)\n\
             0 PRs; 234 more matched the search but were not listed\n"
        );
    }

    #[test]
    fn searches_are_scoped_to_repos() {
        let repos = [
            Repo::parse("jdx/mise").unwrap(),
            Repo::parse("jdx/hk").unwrap(),
        ];
        assert_eq!(
            search("is:open author:@me", &repos),
            Lookup::Search {
                query: "is:pr is:open author:@me repo:jdx/mise repo:jdx/hk".into(),
                after: None,
                listed: 0,
            }
        );
    }
}
