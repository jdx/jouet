//! Checks as `wait ci` and `wait run` see them: check runs, commit statuses
//! and Actions jobs reduced to one shape, selected by name, counted, and
//! judged.

use jouet_github::entire::gates::{self, Gates};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Pass,
    Fail,
    Pending,
    /// Skipped and neutral checks, which count as passing.
    Skipped,
    Cancelled,
}

impl State {
    pub(crate) fn finished(self) -> bool {
        self != Self::Pending
    }

    pub(crate) fn failed(self) -> bool {
        matches!(self, Self::Fail | Self::Cancelled)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Pending => "pending",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    /// A check run's or job's `status` and `conclusion`.
    pub(crate) fn of_run(status: &str, conclusion: Option<&str>) -> Self {
        if status != "completed" {
            return Self::Pending;
        }
        match conclusion.unwrap_or_default() {
            "success" => Self::Pass,
            "skipped" | "neutral" | "stale" => Self::Skipped,
            "cancelled" => Self::Cancelled,
            _ => Self::Fail,
        }
    }

    /// A commit status's `state`.
    pub(crate) fn of_status(state: &str) -> Self {
        match state {
            "success" => Self::Pass,
            "pending" | "expected" => Self::Pending,
            _ => Self::Fail,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Check {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) workflow: Option<String>,
    pub(crate) state: State,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) conclusion: Option<String>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub(crate) required: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) run_id: Option<u64>,
    /// Set for GitHub Actions jobs, whose logs can be fetched.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) job_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) failed_step: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub(crate) log_excerpt: Vec<String>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub(crate) log_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) log_path: Option<String>,
    /// The command that runs this job on this machine, for a failed
    /// Actions job when jactionlint is installed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) local: Option<String>,
    /// For the Entire Gates check, its gates as its summary lists them.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) entire: Option<Gates>,
}

impl Check {
    pub(crate) fn new(name: String, state: State) -> Self {
        Self {
            name,
            workflow: None,
            state,
            conclusion: None,
            required: false,
            url: None,
            run_id: None,
            job_id: None,
            failed_step: None,
            log_excerpt: Vec::new(),
            log_truncated: false,
            log_path: None,
            local: None,
            entire: None,
        }
    }

    /// A failed Entire Gates check that only repeats other failures: failed
    /// checks, or a branch behind or conflicting with its base. It does not
    /// count as a failure of its own.
    pub(crate) fn derived(&self) -> bool {
        self.state.failed() && self.entire.as_ref().is_some_and(Gates::derived)
    }

    /// An entry of `GET /repos/{o}/{r}/commits/{sha}/check-runs`. Actions
    /// check runs share their ID with the job, and link to the run.
    pub(crate) fn from_check_run(v: &Value) -> Option<Self> {
        let name = v["name"].as_str()?.to_string();
        let conclusion = v["conclusion"].as_str();
        let mut check = Self::new(
            name,
            State::of_run(v["status"].as_str().unwrap_or_default(), conclusion),
        );
        check.conclusion = conclusion.map(str::to_string);
        let url = v["html_url"].as_str().or(v["details_url"].as_str());
        check.url = url.map(str::to_string);
        if v["app"]["slug"].as_str() == Some("github-actions") {
            check.job_id = v["id"].as_u64();
            check.run_id = url.and_then(run_id_in_url);
        }
        if check.name == gates::CHECK {
            // Its details are the trail on Entire.
            if let Some(trail) = v["details_url"].as_str() {
                check.url = Some(trail.to_string());
            }
            let output = &v["output"];
            check.entire = Some(Gates::parse(
                v["status"].as_str().unwrap_or_default(),
                conclusion,
                output["title"].as_str(),
                output["summary"].as_str(),
                v["details_url"].as_str(),
            ));
        }
        Some(check)
    }

    /// An entry of a combined commit status's `statuses`.
    pub(crate) fn from_status(v: &Value) -> Option<Self> {
        let state = v["state"].as_str()?;
        let mut check = Self::new(v["context"].as_str()?.to_string(), State::of_status(state));
        if check.state.finished() {
            check.conclusion = Some(state.to_string());
        }
        check.url = v["target_url"].as_str().map(str::to_string);
        Some(check)
    }

    /// An entry of `GET /repos/{o}/{r}/actions/runs/{id}/jobs`.
    pub(crate) fn from_job(v: &Value) -> Option<Self> {
        let conclusion = v["conclusion"].as_str();
        let mut check = Self::new(
            v["name"].as_str()?.to_string(),
            State::of_run(v["status"].as_str().unwrap_or_default(), conclusion),
        );
        check.conclusion = conclusion.map(str::to_string);
        check.workflow = v["workflow_name"].as_str().map(str::to_string);
        check.url = v["html_url"].as_str().map(str::to_string);
        check.run_id = v["run_id"].as_u64();
        check.job_id = v["id"].as_u64();
        Some(check)
    }
}

/// The run ID in an Actions URL such as `.../actions/runs/456/job/123`.
pub(crate) fn run_id_in_url(url: &str) -> Option<u64> {
    let (_, rest) = url.split_once("/actions/runs/")?;
    rest.split(['/', '?', '#']).next()?.parse().ok()
}

/// Which checks a wait looks at.
#[derive(Debug, Default)]
pub struct Selection {
    pub required: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Selection {
    pub(crate) fn selects(&self, check: &Check) -> bool {
        (!self.required || check.required)
            && (self.include.is_empty() || self.include.iter().any(|g| glob(g, &check.name)))
            && !self.exclude.iter().any(|g| glob(g, &check.name))
    }

    /// Whether events about this check name concern this wait. Whether a
    /// check is required is not known from its name alone, so `--required`
    /// does not filter events.
    pub(crate) fn selects_name(&self, name: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|g| glob(g, name)))
            && !self.exclude.iter().any(|g| glob(g, name))
    }
}

/// `*` matches any run of characters and `?` any one; everything else
/// matches itself.
pub(crate) fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    // The classic two-pointer match with backtracking to the last `*`.
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Counts {
    pub(crate) pass: u32,
    pub(crate) fail: u32,
    pub(crate) pending: u32,
    pub(crate) skipped: u32,
    pub(crate) cancelled: u32,
    /// Failed Entire Gates checks that only repeat other failures.
    pub(crate) derived: u32,
}

impl Counts {
    pub(crate) fn of(checks: &[Check]) -> Self {
        let mut c = Self::default();
        for check in checks {
            if check.derived() {
                c.derived += 1;
                continue;
            }
            *match check.state {
                State::Pass => &mut c.pass,
                State::Fail => &mut c.fail,
                State::Pending => &mut c.pending,
                State::Skipped => &mut c.skipped,
                State::Cancelled => &mut c.cancelled,
            } += 1;
        }
        c
    }

    /// `41 pass, 2 fail, 3 skipped, 0 pending`, plus cancelled when any.
    pub(crate) fn summary(&self) -> String {
        let mut s = format!(
            "{} pass, {} fail, {} skipped, {} pending",
            self.pass, self.fail, self.skipped, self.pending
        );
        if self.cancelled > 0 {
            let _ = write!(s, ", {} cancelled", self.cancelled);
        }
        if self.derived > 0 {
            let _ = write!(s, ", {} derived", self.derived);
        }
        s
    }
}

/// Where a set of checks stands.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Success,
    Failure,
    Pending,
}

/// Judges the selected checks: a failure ends the wait at once unless
/// `all`, in which case every check must finish first. A derived failure
/// is not one.
pub(crate) fn verdict(checks: &[Check], all: bool) -> Verdict {
    let failed = checks.iter().any(|c| c.state.failed() && !c.derived());
    let pending = checks.iter().any(|c| !c.state.finished());
    match (failed, pending) {
        (true, false) => Verdict::Failure,
        (true, true) if !all => Verdict::Failure,
        (_, true) => Verdict::Pending,
        (false, false) => Verdict::Success,
    }
}

/// The block of text for each failed check: its name, step, URL and log
/// excerpt.
pub(crate) fn failure_blocks(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks.iter().filter(|c| c.state.failed() && !c.derived()) {
        let word = if c.state == State::Cancelled {
            "CANCELLED"
        } else {
            "FAIL"
        };
        let _ = write!(out, "\n{word} {}", c.name);
        if let Some(step) = &c.failed_step {
            let _ = write!(out, "  step \"{step}\"");
        }
        if c.required {
            out.push_str("  required");
        }
        out.push('\n');
        if let Some(url) = &c.url {
            let _ = writeln!(out, "  {url}");
        }
        for line in &c.log_excerpt {
            let _ = writeln!(out, "  {line}");
        }
        if let Some(path) = &c.log_path {
            let _ = writeln!(out, "  full log: {path}");
        }
        if let Some(local) = &c.local {
            let _ = writeln!(out, "  locally: {local}");
        }
        if let Some(g) = &c.entire {
            let _ = writeln!(out, "  {}", g.failing_text());
        }
    }
    out
}

/// Fills in, for each failed Actions job, the `jouet ci run` command that
/// runs it on this machine. `workflow` names the workflow of a job that
/// does not know its own.
pub(crate) fn suggest_local(checks: &mut [Check], workflow: Option<&str>) {
    for c in checks
        .iter_mut()
        .filter(|c| c.state.failed() && c.job_id.is_some())
    {
        let mut cmd = String::from("jouet ci run");
        if let Some(workflow) = c.workflow.as_deref().or(workflow) {
            cmd.push(' ');
            cmd.push_str(&shell_quote(workflow));
        }
        cmd.push_str(" --check ");
        cmd.push_str(&shell_quote(&c.name));
        c.local = Some(cmd);
    }
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
    use serde_json::json;

    #[test]
    fn globs() {
        assert!(glob("unit (*)", "unit (ubuntu-latest)"));
        assert!(glob("*lint*", "hk lint"));
        assert!(glob("a?c", "abc"));
        assert!(glob("*", ""));
        assert!(!glob("unit", "unit (macos)"));
        assert!(!glob("a*b*c", "abcb"));
        assert!(glob("a*b*c", "axxbyyc"));
    }

    #[test]
    fn check_runs_from_actions_know_their_job_and_run() {
        let v = json!({
            "id": 123456, "name": "unit", "status": "completed", "conclusion": "failure",
            "html_url": "https://github.com/jdx/mise/actions/runs/456/job/123456",
            "app": { "slug": "github-actions" }
        });
        let c = Check::from_check_run(&v).unwrap();
        assert_eq!(c.state, State::Fail);
        assert_eq!((c.run_id, c.job_id), (Some(456), Some(123456)));
        let v = json!({ "id": 9, "name": "codecov", "status": "in_progress", "app": { "slug": "codecov" } });
        let c = Check::from_check_run(&v).unwrap();
        assert_eq!((c.state, c.job_id), (State::Pending, None));
    }

    #[test]
    fn skipped_and_neutral_count_as_passing_and_failures_end_early() {
        let check = |state| Check::new("x".into(), state);
        let pass = [check(State::Pass), check(State::Skipped)];
        assert_eq!(verdict(&pass, false), Verdict::Success);
        let early = [check(State::Fail), check(State::Pending)];
        assert_eq!(verdict(&early, false), Verdict::Failure);
        assert_eq!(verdict(&early, true), Verdict::Pending);
        assert_eq!(verdict(&[check(State::Cancelled)], true), Verdict::Failure);
        assert_eq!(
            Counts::of(&early).summary(),
            "0 pass, 1 fail, 0 skipped, 1 pending"
        );
    }

    #[test]
    fn selection_by_glob_and_required() {
        let mut c = Check::new("unit (macos)".into(), State::Pass);
        let sel = Selection {
            include: vec!["unit*".into()],
            exclude: vec!["*windows*".into()],
            ..Selection::default()
        };
        assert!(sel.selects(&c));
        let required = Selection {
            required: true,
            ..Selection::default()
        };
        assert!(!required.selects(&c));
        c.required = true;
        assert!(required.selects(&c));
    }

    /// The Entire Gates check run as GitHub lists it, recorded from
    /// jdx/mise#15220 (checks) and jdx/mise#13958 (checks and findings).
    fn entire_gates(summary: &str) -> Check {
        Check::from_check_run(&json!({
            "id": 124530630313u64, "name": "Entire Gates", "status": "completed",
            "conclusion": "failure", "app": { "slug": "entire" },
            "html_url": "https://github.com/jdx/mise/runs/124530630313",
            "details_url": "https://entire.io/gh/jdx/mise/trails/424",
            "output": { "title": "1 blocking gate not passing", "summary": summary },
        }))
        .unwrap()
    }

    #[test]
    fn entire_gates_failing_on_other_checks_is_derived() {
        let gates = entire_gates(
            "- ❌ **Checks**: failed — CI checks have not passed\n- ✅ **Findings**: passed — Agent review found no blocking issues\n\n[Trail #1176 on Entire](https://entire.com/gh/jdx/-/trails/3261/changes/mise/1176)",
        );
        assert!(gates.derived());
        assert_eq!(
            gates.url.as_deref(),
            Some("https://entire.io/gh/jdx/mise/trails/424")
        );
        let mut release = Check::new("release".into(), State::Fail);
        release.job_id = Some(1);
        // A failed check alongside is the failure; Entire Gates is not another.
        let both = [gates.clone(), release];
        assert_eq!(verdict(&both, false), Verdict::Failure);
        assert_eq!(
            Counts::of(&both).summary(),
            "0 pass, 1 fail, 0 skipped, 0 pending, 1 derived"
        );
        assert!(!failure_blocks(&both).contains("FAIL Entire Gates"));
        // While the others rerun, a stale Entire Gates failure ends nothing.
        let rerunning = [gates.clone(), Check::new("release".into(), State::Pending)];
        assert_eq!(verdict(&rerunning, false), Verdict::Pending);
        let behind = entire_gates(
            "- ✅ **Checks**: passed — CI checks have passed\n- ❌ **Up to date**: failed — 3 commits behind · Merge conflicts",
        );
        assert_eq!(
            verdict(&[behind, Check::new("test".into(), State::Pass)], false),
            Verdict::Success
        );
    }

    #[test]
    fn entire_gates_failing_on_findings_is_a_failure() {
        let gates = entire_gates(
            "- ❌ **Checks**: failed — CI checks have not passed\n- ❌ **Findings**: failed — Unresolved findings: 2 medium severity",
        );
        assert!(!gates.derived());
        assert_eq!(
            verdict(std::slice::from_ref(&gates), false),
            Verdict::Failure
        );
        assert!(failure_blocks(&[gates]).contains(
            "FAIL Entire Gates\n  https://entire.io/gh/jdx/mise/trails/424\n  checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity\n"
        ));
    }

    #[test]
    fn a_failed_actions_job_gets_a_local_command() {
        let mut failed = Check::new("unit (ubuntu-latest, 1.23)".into(), State::Fail);
        failed.job_id = Some(7);
        let mut named = Check::new("lint".into(), State::Fail);
        named.job_id = Some(8);
        named.workflow = Some("CI".into());
        let mut external = Check::new("codecov/patch".into(), State::Fail);
        external.job_id = None;
        let mut passed = Check::new("build".into(), State::Pass);
        passed.job_id = Some(9);
        let mut checks = vec![failed, named, external, passed];
        suggest_local(&mut checks, Some("Run's workflow"));
        assert_eq!(
            checks[0].local.as_deref(),
            Some("jouet ci run 'Run'\\''s workflow' --check 'unit (ubuntu-latest, 1.23)'")
        );
        assert_eq!(
            checks[1].local.as_deref(),
            Some("jouet ci run CI --check lint")
        );
        assert_eq!(checks[2].local, None, "not an Actions job");
        assert_eq!(checks[3].local, None, "it passed");
        assert!(failure_blocks(&checks).contains("  locally: jouet ci run CI --check lint\n"));
    }
}
