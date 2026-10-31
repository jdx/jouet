//! The verdict on one failure, from its signature and what history showed.
//! Pure, so each rule is tested on its own.

use super::signature::Signature;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    Flaky,
    Infrastructure,
    Unknown,
    Real,
}

impl Verdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Flaky => "flaky",
            Self::Infrastructure => "infrastructure",
            Self::Unknown => "unknown",
            Self::Real => "real",
        }
    }

    /// Whether running the job again can pass without a change.
    pub(crate) fn rerunnable(self) -> bool {
        matches!(self, Self::Flaky | Self::Infrastructure)
    }
}

/// A run somewhere in history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) workflow: String,
    pub(crate) branch: String,
    pub(crate) sha: String,
    pub(crate) run_id: u64,
    pub(crate) attempt: u64,
}

impl Place {
    fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(8)]
    }
}

/// The same job in another attempt or run on the same commit.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub(crate) place: Place,
    pub(crate) passed: bool,
    /// For a failure: whether it failed the same way.
    pub(crate) same: bool,
}

/// What history showed about one failure.
#[derive(Debug, Default)]
pub(crate) struct Facts {
    pub(crate) os: Option<&'static str>,
    /// This run is on the default branch.
    pub(crate) on_main: bool,
    pub(crate) main_branch: String,
    pub(crate) same_commit: Vec<Seen>,
    /// Runs on the default branch that failed the same way, newest first.
    pub(crate) main_failed: Vec<Place>,
    /// A later run of the newest match's workflow that passed on main.
    pub(crate) main_green_since: Option<Place>,
    /// Jobs on this commit that passed the failed tests: name and OS.
    pub(crate) passed_on_commit: Vec<(String, Option<&'static str>)>,
    /// Other commits that failed the same way, and whether their branch
    /// passed later.
    pub(crate) elsewhere: Vec<(Place, bool)>,
    /// Other runs of the workflow looked at, for an `unknown`.
    pub(crate) looked_at: usize,
    pub(crate) workflow: String,
}

pub(crate) struct Judged {
    pub(crate) verdict: Verdict,
    pub(crate) evidence: Vec<String>,
    pub(crate) main_broken: bool,
}

fn places(list: &[Place]) -> String {
    let parts: Vec<String> = list
        .iter()
        .map(|p| format!("{} at {} (run {})", p.workflow, p.short(), p.run_id))
        .collect();
    join_and(&parts)
}

pub(crate) fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn what(sig: &Signature) -> String {
    match sig.tests.as_slice() {
        [] => "the step".into(),
        [one] => format!("`{}`", one.name),
        many => format!("{} tests", many.len()),
    }
}

pub(crate) fn judge(sig: &Signature, f: &Facts) -> Judged {
    let mut evidence = Vec::new();
    let done = |verdict, evidence, main_broken| Judged {
        verdict,
        evidence,
        main_broken,
    };
    if let Some(infra) = sig.infrastructure() {
        let cause = match infra.cause {
            "runner" => "the runner failed",
            "disk" => "the runner ran out of disk",
            "rate_limit" => "a rate limit",
            "network" => "a network failure",
            "registry" => "a registry or download server failed",
            "service" => "a service container failed",
            _ => "the cache service failed",
        };
        evidence.push(format!("{cause}: {}", infra.line));
        return done(Verdict::Infrastructure, evidence, false);
    }
    if !sig.errors.is_empty() {
        evidence.push("a compile or lint error, which fails the same way on every run".into());
        let mut main_broken = false;
        if f.on_main {
            evidence.push(format!(
                "this run is on {}: {} is broken",
                f.main_branch, f.main_branch
            ));
            main_broken = true;
        } else if !f.main_failed.is_empty() {
            evidence.push(format!(
                "{} is broken, not this change: the same error failed {} on {}",
                f.main_branch,
                places(&f.main_failed),
                f.main_branch
            ));
            main_broken = true;
        }
        if let Some(green) = &f.main_green_since {
            evidence.push(format!(
                "{} has passed since ({} at {}, run {}): merge {} in",
                f.main_branch,
                green.workflow,
                green.short(),
                green.run_id,
                f.main_branch
            ));
        }
        return done(Verdict::Real, evidence, main_broken);
    }
    if let Some(seen) = f.same_commit.iter().find(|s| s.passed) {
        evidence.push(format!(
            "passed on this commit in run {} attempt {}",
            seen.place.run_id, seen.place.attempt
        ));
        return done(Verdict::Flaky, evidence, false);
    }
    let again = f.same_commit.iter().filter(|s| s.same).count();
    if again > 0 {
        evidence.push(format!(
            "{} failed the same way on every attempt on this commit ({} of {})",
            what(sig),
            again + 1,
            again + 1
        ));
        return done(Verdict::Real, evidence, false);
    }
    if let Some(latest) = f.main_failed.first() {
        evidence.push(format!(
            "{} is broken, not this change: {} fails the same way in {} on {}",
            f.main_branch,
            what(sig),
            places(std::slice::from_ref(latest)),
            f.main_branch
        ));
        return done(Verdict::Real, evidence, true);
    }
    if !f.passed_on_commit.is_empty() {
        let other_os: Vec<&'static str> = f
            .passed_on_commit
            .iter()
            .filter_map(|(_, os)| *os)
            .filter(|os| Some(*os) != f.os)
            .fold(Vec::new(), |mut v, os| {
                if !v.contains(&os) {
                    v.push(os);
                }
                v
            });
        let jobs: Vec<String> = f
            .passed_on_commit
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        match (f.os, other_os.is_empty()) {
            (Some(os), false) => {
                evidence.push(format!(
                    "passed on {} on this commit ({}); failed on {os}",
                    other_os.join(", "),
                    join_and(&jobs)
                ));
                evidence.push(format!(
                    "a failure on one OS can be a real {os} bug: if a rerun fails the same way on this commit, it is real"
                ));
            }
            _ => evidence.push(format!(
                "{} passed on this commit in {}",
                what(sig),
                join_and(&jobs)
            )),
        }
        return done(Verdict::Flaky, evidence, false);
    }
    let green: Vec<&Place> = f
        .elsewhere
        .iter()
        .filter(|(_, green)| *green)
        .map(|(p, _)| p)
        .collect();
    if !green.is_empty() {
        let parts: Vec<String> = green
            .iter()
            .map(|p| format!("{} at {} (run {})", p.branch, p.short(), p.run_id))
            .collect();
        evidence.push(format!(
            "known flaky: {} also failed on {}, which passed later",
            what(sig),
            join_and(&parts)
        ));
        return done(Verdict::Flaky, evidence, false);
    }
    // Not enough to go on: say what was looked at.
    if sig.tests.is_empty() {
        evidence.push(
            "no failed test, compile error or runner trouble recognised in the step's output"
                .into(),
        );
    }
    if f.same_commit.is_empty() {
        evidence.push("no other run of this job on this commit".into());
    } else {
        evidence.push(format!(
            "it failed differently in {} other run(s) on this commit",
            f.same_commit.len()
        ));
    }
    if !f.elsewhere.is_empty() {
        let parts: Vec<String> = f
            .elsewhere
            .iter()
            .map(|(p, _)| format!("{} at {} (run {})", p.branch, p.short(), p.run_id))
            .collect();
        evidence.push(format!(
            "it also failed on {}, which has not passed since",
            join_and(&parts)
        ));
    } else if !sig.tests.is_empty() {
        evidence.push(format!(
            "{} did not fail in the {} other run(s) of {} looked at",
            what(sig),
            f.looked_at,
            f.workflow
        ));
    }
    done(Verdict::Unknown, evidence, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::explain::signature::{Infra, Test};

    fn place(branch: &str, run_id: u64) -> Place {
        Place {
            workflow: "ci".into(),
            branch: branch.into(),
            sha: "e10984c4321d69a5".into(),
            run_id,
            attempt: 1,
        }
    }

    fn test_sig(name: &str) -> Signature {
        Signature {
            tests: vec![Test {
                framework: "cargo",
                name: name.into(),
            }],
            ..Signature::default()
        }
    }

    fn facts() -> Facts {
        Facts {
            os: Some("macos"),
            main_branch: "main".into(),
            workflow: "ci".into(),
            ..Facts::default()
        }
    }

    #[test]
    fn a_test_that_passed_on_another_os_is_flaky_and_says_where() {
        let mut f = facts();
        f.passed_on_commit = vec![
            ("trusted / test".into(), Some("linux")),
            ("trusted / test-windows".into(), Some("windows")),
        ];
        let j = judge(&test_sig("a"), &f);
        assert_eq!(j.verdict, Verdict::Flaky);
        assert_eq!(
            j.evidence[0],
            "passed on linux, windows on this commit (trusted / test and trusted / test-windows); failed on macos"
        );
        assert!(j.evidence[1].contains("if a rerun fails the same way on this commit, it is real"));
    }

    #[test]
    fn failing_again_on_the_same_commit_makes_it_real() {
        let mut f = facts();
        f.passed_on_commit = vec![("trusted / test".into(), Some("linux"))];
        f.same_commit = vec![Seen {
            place: place("x", 1),
            passed: false,
            same: true,
        }];
        let j = judge(&test_sig("a"), &f);
        assert_eq!(j.verdict, Verdict::Real);
        assert_eq!(
            j.evidence,
            ["`a` failed the same way on every attempt on this commit (2 of 2)"]
        );
    }

    #[test]
    fn a_rerun_that_went_green_is_flaky() {
        let mut f = facts();
        f.same_commit = vec![Seen {
            place: place("x", 7),
            passed: true,
            same: false,
        }];
        assert_eq!(judge(&test_sig("a"), &f).verdict, Verdict::Flaky);
    }

    #[test]
    fn a_compile_error_is_real_and_says_main_is_broken() {
        let mut f = facts();
        f.main_failed = vec![place("main", 2), {
            let mut p = place("main", 3);
            p.workflow = "docs".into();
            p
        }];
        f.main_green_since = Some(place("main", 4));
        let sig = Signature {
            errors: vec!["error[E0425]: x".into()],
            ..Signature::default()
        };
        let j = judge(&sig, &f);
        assert_eq!(j.verdict, Verdict::Real);
        assert!(j.main_broken);
        assert_eq!(
            j.evidence[1],
            "main is broken, not this change: the same error failed ci at e10984c4 (run 2) and docs at e10984c4 (run 3) on main"
        );
        assert_eq!(
            j.evidence[2],
            "main has passed since (ci at e10984c4, run 4): merge main in"
        );
    }

    #[test]
    fn a_test_failing_on_main_is_real_and_known_flakes_are_flaky() {
        let mut f = facts();
        f.main_failed = vec![place("main", 2)];
        let j = judge(&test_sig("a"), &f);
        assert_eq!((j.verdict, j.main_broken), (Verdict::Real, true));

        let mut f = facts();
        f.elsewhere = vec![(place("feat/x", 5), true)];
        let j = judge(&test_sig("a"), &f);
        assert_eq!(j.verdict, Verdict::Flaky);
        assert_eq!(
            j.evidence,
            ["known flaky: `a` also failed on feat/x at e10984c4 (run 5), which passed later"]
        );
    }

    #[test]
    fn thin_evidence_is_unknown_and_says_what_was_looked_at() {
        let mut f = facts();
        f.looked_at = 4;
        let j = judge(&test_sig("a"), &f);
        assert_eq!(j.verdict, Verdict::Unknown);
        assert_eq!(
            j.evidence,
            [
                "no other run of this job on this commit",
                "`a` did not fail in the 4 other run(s) of ci looked at"
            ]
        );
        let j = judge(&Signature::default(), &facts());
        assert_eq!(j.verdict, Verdict::Unknown);
    }

    #[test]
    fn runner_trouble_is_infrastructure() {
        let sig = Signature {
            infra: Some(Infra {
                cause: "disk",
                line: "No space left on device".into(),
                strong: true,
            }),
            ..test_sig("a")
        };
        let j = judge(&sig, &facts());
        assert_eq!(j.verdict, Verdict::Infrastructure);
        assert_eq!(
            j.evidence,
            ["the runner ran out of disk: No space left on device"]
        );
    }
}
