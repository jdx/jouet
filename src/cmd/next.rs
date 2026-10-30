//! `jouet pr next`.

use super::Output;
use super::pr::{self, Selection};
use crate::NextArgs;
use jouet_github::Client;
use jouet_github::pr::next::{self, Kind, Next};
use jouet_github::pr::status::{Incomplete, PrStatus};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;

#[derive(Debug, Serialize)]
struct Report {
    prs: Vec<Entry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    not_found: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    incomplete: Vec<Incomplete>,
}

#[derive(Debug, Serialize)]
struct Entry {
    repo: String,
    number: u64,
    title: String,
    url: String,
    #[serde(flatten)]
    next: Next,
}

impl Entry {
    fn of(pr: PrStatus) -> Self {
        let next = next::next(&pr);
        Self {
            repo: pr.repo,
            number: pr.number,
            title: pr.title,
            url: pr.url,
            next,
        }
    }
}

pub(crate) fn run(args: &NextArgs, out: &Output) -> Exit {
    let settle = match pr::settle_arg(args.settle.as_deref()) {
        Ok(settle) => settle,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let selection = Selection {
        prs: &args.prs,
        mine: args.mine,
        search: args.search.as_deref(),
        repo: &args.repo,
    };
    let lookups = match pr::lookups(&selection) {
        Ok(lookups) => lookups,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let fetched = match pr::fetch(&mut Client::from_env(), lookups, settle) {
        Ok(report) => report,
        Err(e) => return out.error(Exit::Remote, &e),
    };
    let report = Report {
        prs: fetched.prs.into_iter().map(Entry::of).collect(),
        not_found: fetched.not_found,
        incomplete: fetched.incomplete,
    };
    out.result(&report, || render(&report));
    exit(&report)
}

/// 3 when a PR was not found, else 10 when any PR needs the agent, else 20
/// when any is waiting, else 0.
fn exit(report: &Report) -> Exit {
    let any = |kind| report.prs.iter().any(|e| e.next.kind == kind);
    if !report.not_found.is_empty() {
        Exit::NotFound
    } else if any(Kind::Act) {
        Exit::Act
    } else if any(Kind::Wait) {
        Exit::Waiting
    } else {
        Exit::Ok
    }
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Done => "done",
        Kind::Act => "act",
        Kind::Wait => "wait",
    }
}

/// Two lines per PR, the action and its reason, then the command; a line
/// naming any that were not found; then the summary.
fn render(report: &Report) -> String {
    let one_repo = report.prs.windows(2).all(|w| w[0].repo == w[1].repo);
    let name = |e: &Entry| {
        if one_repo {
            format!("#{}", e.number)
        } else {
            format!("{}#{}", e.repo, e.number)
        }
    };
    let width = report
        .prs
        .iter()
        .map(|e| name(e).chars().count())
        .max()
        .unwrap_or(0);
    let mut s = String::new();
    for e in &report.prs {
        let _ = writeln!(
            s,
            "{:<width$} {:<4} {}: {}",
            name(e),
            kind_word(e.next.kind),
            e.next.action.as_str(),
            e.next.reason
        );
        if let Some(command) = &e.next.command {
            let _ = writeln!(s, "{:<width$} next: {command}", "");
        }
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
    let mut parts = Vec::new();
    for (kind, label) in [
        (Kind::Act, "to act on"),
        (Kind::Wait, "waiting"),
        (Kind::Done, "done"),
    ] {
        let names: Vec<String> = report
            .prs
            .iter()
            .filter(|e| e.next.kind == kind)
            .map(&name)
            .collect();
        if !names.is_empty() {
            parts.push(format!("{} {label} ({})", names.len(), names.join(", ")));
        }
    }
    if !parts.is_empty() {
        let _ = write!(s, ": {}", parts.join(", "));
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

#[cfg(test)]
mod tests {
    use super::*;
    use jouet_github::pr::next::Action;

    fn entry(repo: &str, number: u64, action: Action, command: Option<&str>) -> Entry {
        Entry {
            repo: repo.into(),
            number,
            title: "t".into(),
            url: String::new(),
            next: Next {
                action,
                kind: action.kind(),
                reason: "why".into(),
                command: command.map(str::to_string),
            },
        }
    }

    fn report(prs: Vec<Entry>, not_found: &[&str]) -> Report {
        Report {
            prs,
            not_found: not_found.iter().map(|s| (*s).to_string()).collect(),
            incomplete: Vec::new(),
        }
    }

    #[test]
    fn the_most_urgent_kind_decides_the_exit() {
        let done = || entry("jdx/a", 1, Action::Merged, None);
        let wait = || entry("jdx/a", 2, Action::ChecksPending, Some("jouet wait ci"));
        let act = || entry("jdx/a", 3, Action::Conflict, Some("git merge"));
        assert_eq!(exit(&report(vec![done()], &[])), Exit::Ok);
        assert_eq!(exit(&report(vec![done(), wait()], &[])), Exit::Waiting);
        assert_eq!(exit(&report(vec![wait(), act()], &[])), Exit::Act);
        assert_eq!(exit(&report(vec![act()], &["jdx/a#9"])), Exit::NotFound);
    }

    #[test]
    fn each_pr_gets_its_action_then_its_command() {
        let r = report(
            vec![
                entry(
                    "jdx/a",
                    19,
                    Action::ChecksPending,
                    Some("jouet wait ci jdx/a#19"),
                ),
                entry("jdx/a", 7, Action::Merged, None),
            ],
            &[],
        );
        assert_eq!(
            render(&r),
            "#19 wait checks_pending: why\n    \
             next: jouet wait ci jdx/a#19\n\
             #7  done merged: why\n\
             2 PRs: 1 waiting (#19), 1 done (#7)\n"
        );
    }
}
