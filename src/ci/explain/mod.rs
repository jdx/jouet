//! `jouet ci explain`: whether a failed GitHub Actions job is flaky,
//! infrastructure or real, from its log and from the history of the same
//! workflow, so that an agent reruns only what a rerun can fix.
//!
//! Each failed job's log is read for a [signature](signature): the tests
//! that failed, compile and lint errors, runner and network trouble. A job
//! that only failed because another did (a `needs:` job reporting the
//! result, a fail-fast cancel) follows that job and gets no verdict of its
//! own. Jobs with the same signature are judged once, from what history
//! shows of it, in this order:
//!
//! 1. the same job on the same commit, in another attempt or run: passed is
//!    flaky, failed the same way again is real;
//! 2. the default branch: failing the same way there is real, and not the
//!    change's fault (for a compile error, across every workflow);
//! 3. the other jobs of the run: a test that passed on another OS on this
//!    commit is flaky, the evidence naming the OSes;
//! 4. the workflow's other recent runs: a test that failed on another
//!    branch that passed later is a known flake.
//!
//! Anything else is `unknown`. Every list is one page and the logs read
//! are counted, so an explain costs a bounded number of requests, and
//! [the cache](cache) makes the next one cheaper.

mod cache;
mod github;
pub(crate) mod signature;
pub(crate) mod verdict;

use super::Refusal;
use github::{BRIEF, Gh};
use jouet_github::entire::Gates;
use jouet_github::pr::target::Repo;
use jouet_github::{Client, Failure};
use jouet_output::exit::Exit;
use jouet_wait::joblog::{self, Step};
use serde::Serialize;
use serde_json::Value;
use signature::Signature;
use std::collections::BTreeMap;
use verdict::{Facts, Place, Seen, Verdict};

/// Failed jobs of one run whose logs are read; the rest are `unknown`.
const OWN_LOGS: usize = 20;
/// Runs of the same workflow listed for history.
const WORKFLOW_RUNS: u32 = 50;
/// Runs of the workflow on the same commit listed.
const SAME_COMMIT_RUNS: u32 = 20;
/// Recent passing runs on the default branch listed, across workflows, to
/// say whether it has passed since a match.
const MAIN_RUNS: u32 = 30;
/// The window of default-branch failures a compile error is compared with,
/// around the time the run started.
const MAIN_BEFORE: i64 = 6 * 60 * 60;
const MAIN_AFTER: i64 = 15 * 60;
/// Other runs whose jobs are read for history, per explain.
const HISTORY_RUNS: u32 = 6;
/// Logs of other jobs and runs read for history, per explain.
const HISTORY_LOGS: u32 = 8;
/// Of those, logs of jobs that passed on the same commit.
const SIBLING_LOGS: usize = 3;
/// Failed runs on the default branch compared with a compile error.
const MAIN_SCAN: usize = 3;

pub(crate) enum Target {
    Pr(jouet_github::pr::target::Target),
    Run(Repo, u64),
    Job(Repo, u64),
}

pub(crate) struct Request {
    pub(crate) target: Target,
    pub(crate) rerun: bool,
    pub(crate) log_lines: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pr: Option<u64>,
    /// `passed`, `pending` (nothing failed yet), or the worst verdict:
    /// `real`, `unknown`, `infrastructure` or `flaky`.
    pub(crate) result: &'static str,
    pub(crate) runs: Vec<RunReport>,
    /// The Entire Gates check on the head, when it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) entire_gates: Option<EntireNote>,
    pub(crate) rerun: Rerun,
    pub(crate) github: Calls,
}

#[derive(Debug, Serialize)]
pub(crate) struct Calls {
    pub(crate) requests: u32,
    pub(crate) cached: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunReport {
    pub(crate) run_id: u64,
    pub(crate) workflow: String,
    pub(crate) head_sha: String,
    pub(crate) head_branch: String,
    pub(crate) attempt: u64,
    pub(crate) url: String,
    /// The PRs the run is for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) prs: Vec<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) in_progress: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) jobs: Vec<JobReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) following: Vec<Following>,
}

#[derive(Debug, Serialize)]
pub(crate) struct JobReport {
    pub(crate) name: String,
    pub(crate) job_id: u64,
    pub(crate) url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) os: Option<&'static str>,
    pub(crate) verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failed_step: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tests: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) infrastructure: Option<signature::Infra>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) main_broken: bool,
    pub(crate) evidence: Vec<String>,
    /// Jobs reported together because they failed the same way: the index
    /// of the first, whose evidence this shares.
    #[serde(skip)]
    pub(crate) group: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) log_excerpt: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) log_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) log_path: Option<String>,
}

/// A job that failed or was cancelled only because another one failed.
#[derive(Debug, Serialize)]
pub(crate) struct Following {
    pub(crate) name: String,
    pub(crate) job_id: u64,
    pub(crate) conclusion: String,
    /// The failed job it follows.
    pub(crate) follows: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Rerun {
    pub(crate) allowed: bool,
    pub(crate) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) jobs: Vec<String>,
    /// The rerun was started.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) started: bool,
}

impl Report {
    pub(crate) fn exit(&self) -> Exit {
        if self.rerun.started {
            return Exit::Ok;
        }
        match self.result {
            "passed" | "flaky" | "infrastructure" => Exit::Ok,
            // Nothing to explain yet: `wait run` first.
            "pending" => Exit::Precondition,
            _ => Exit::Failed,
        }
    }
}

/// A run, as the runs API describes it.
#[derive(Debug, Clone)]
struct Run {
    id: u64,
    workflow: String,
    workflow_id: u64,
    head_sha: String,
    head_branch: String,
    attempt: u64,
    status: String,
    conclusion: String,
    created_at: String,
    url: String,
    prs: Vec<u64>,
}

impl Run {
    fn from(v: &Value) -> Option<Self> {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Some(Self {
            id: v["id"].as_u64()?,
            workflow: s("name"),
            workflow_id: v["workflow_id"].as_u64().unwrap_or_default(),
            head_sha: s("head_sha"),
            head_branch: s("head_branch"),
            attempt: v["run_attempt"].as_u64().unwrap_or(1),
            status: s("status"),
            conclusion: s("conclusion"),
            created_at: s("created_at"),
            url: s("html_url"),
            prs: v["pull_requests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["number"].as_u64())
                .collect(),
        })
    }

    fn completed(&self) -> bool {
        self.status == "completed"
    }

    fn failed(&self) -> bool {
        matches!(self.conclusion.as_str(), "failure" | "timed_out")
    }

    fn place(&self) -> Place {
        Place {
            workflow: self.workflow.clone(),
            branch: self.head_branch.clone(),
            sha: self.head_sha.clone(),
            run_id: self.id,
            attempt: self.attempt,
        }
    }
}

/// A job of a run.
#[derive(Debug, Clone)]
struct Job {
    id: u64,
    name: String,
    conclusion: String,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    url: String,
    os: Option<&'static str>,
    doc: Value,
}

impl Job {
    fn from(v: &Value) -> Option<Self> {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let name = s("name");
        let labels: Vec<&str> = v["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        Some(Self {
            id: v["id"].as_u64()?,
            os: os_of(&labels.join(" ")).or_else(|| os_of(&name)),
            name,
            conclusion: s("conclusion"),
            started_at: v["started_at"]
                .as_str()
                .and_then(jouet_output::time::parse_utc),
            completed_at: v["completed_at"]
                .as_str()
                .and_then(jouet_output::time::parse_utc),
            url: s("html_url"),
            doc: v.clone(),
        })
    }

    fn failed(&self) -> bool {
        matches!(
            self.conclusion.as_str(),
            "failure" | "timed_out" | "startup_failure"
        )
    }

    fn passed(&self) -> bool {
        self.conclusion == "success"
    }
}

/// The OS a runner label or job name names.
fn os_of(text: &str) -> Option<&'static str> {
    let text = text.to_ascii_lowercase();
    [
        ("windows", "windows"),
        ("macos", "macos"),
        ("osx", "macos"),
        ("ubuntu", "linux"),
        ("linux", "linux"),
    ]
    .into_iter()
    .find(|(word, _)| text.contains(word))
    .map(|(_, os)| os)
}

/// A failed job, read.
struct Read {
    job: Job,
    step: Option<String>,
    sig: Signature,
    excerpt: joblog::Excerpt,
    log_path: Option<String>,
    /// Why there is no signature to go on, when the log was not read.
    unread: Option<String>,
}

/// What history is shared between the jobs and runs of one explain.
struct History {
    main: Option<String>,
    workflow_runs: BTreeMap<u64, Vec<Run>>,
    runs_left: u32,
    logs_left: u32,
}

fn remote(f: Failure) -> Refusal {
    Refusal {
        exit: Exit::Remote,
        message: f.message(),
        hint: None,
    }
}

fn refusal(exit: Exit, message: impl Into<String>) -> Refusal {
    Refusal {
        exit,
        message: message.into(),
        hint: None,
    }
}

pub(crate) fn explain(client: &mut Client, req: &Request) -> Result<Report, Refusal> {
    let cache_root = jouet_output::paths::state_dir().map(|d| d.join("ci"));
    let (repo, pr, runs, only_job) = match &req.target {
        Target::Pr(t) => {
            let (repo, n) = jouet_wait::ci::resolve(client, t.clone()).map_err(|h| match h {
                jouet_wait::remote::Halt::Stop(s) => refusal(s.exit, s.message),
                jouet_wait::remote::Halt::RateLimited { reset } => {
                    remote(Failure::RateLimited { reset })
                }
            })?;
            (repo, Some(n), None, None)
        }
        Target::Run(repo, id) => (repo.clone(), None, Some(*id), None),
        Target::Job(repo, id) => (repo.clone(), None, None, Some(*id)),
    };
    let mut gh = Gh::new(client, repo.clone(), cache_root);
    let mut pr_doc = None;
    let mut runs: Vec<Run> = match (pr, runs, only_job) {
        (Some(n), _, _) => {
            let (doc, runs) = pr_runs(&mut gh, n)?;
            pr_doc = Some(doc);
            runs
        }
        (_, Some(id), _) => vec![run(&mut gh, id)?],
        (_, _, Some(job)) => {
            let doc = gh
                .json(&format!("/actions/jobs/{job}"), Some(BRIEF))
                .map_err(remote)?
                .ok_or_else(|| refusal(Exit::NotFound, format!("no job {job} in {repo}")))?;
            let id = doc["run_id"].as_u64().unwrap_or_default();
            vec![run(&mut gh, id)?]
        }
        _ => Vec::new(),
    };
    runs.sort_by(|a, b| a.workflow.cmp(&b.workflow));
    let mut history = History {
        main: None,
        workflow_runs: BTreeMap::new(),
        runs_left: HISTORY_RUNS,
        logs_left: HISTORY_LOGS,
    };
    let mut reports = Vec::new();
    for run in &runs {
        reports.push(explain_run(
            &mut gh,
            &mut history,
            run,
            only_job,
            req.log_lines,
        )?);
    }
    let failed: Vec<&JobReport> = reports.iter().flat_map(|r| &r.jobs).collect();
    let in_progress = reports.iter().any(|r| r.in_progress);
    let result = match failed.iter().map(|j| j.verdict).max() {
        Some(v) => v.as_str(),
        None if in_progress => "pending",
        None => "passed",
    };
    let again = match (&req.target, pr) {
        (Target::Job(_, id), _) => format!("--job {id} --repo {repo}"),
        (_, Some(n)) => format!("{repo}#{n}"),
        _ => runs
            .first()
            .map(|r| format!("--run {} --repo {repo}", r.id))
            .unwrap_or_default(),
    };
    let entire = match (pr, &pr_doc) {
        (Some(n), Some(doc)) => entire_gates(&mut gh, n, doc, !failed.is_empty()),
        _ => None,
    };
    let mut rerun = rerun_plan(&reports, &again, in_progress);
    if req.rerun {
        if !rerun.allowed {
            return Err(Refusal {
                exit: Exit::Precondition,
                message: format!("not rerunning: {}", rerun.reason),
                hint: None,
            });
        }
        for run in runs.iter().filter(|r| {
            reports
                .iter()
                .any(|rep| rep.run_id == r.id && !rep.jobs.is_empty())
        }) {
            let path = match only_job {
                Some(job) => format!("/actions/jobs/{job}/rerun"),
                None => format!("/actions/runs/{}/rerun-failed-jobs", run.id),
            };
            gh.rerun(&path, run.id).map_err(remote)?;
        }
        rerun.started = true;
    }
    Ok(Report {
        repo: repo.to_string(),
        pr,
        result,
        runs: reports,
        entire_gates: entire,
        rerun,
        github: Calls {
            requests: gh.fetched,
            cached: gh.cached,
        },
    })
}

/// A run by its ID; a run still going is not cached.
fn run(gh: &mut Gh<'_>, id: u64) -> Result<Run, Refusal> {
    let repo = gh.repo.to_string();
    let not_found = || refusal(Exit::NotFound, format!("no run {id} in {repo}"));
    let doc = gh
        .json(&format!("/actions/runs/{id}"), Some(BRIEF))
        .map_err(remote)?
        .ok_or_else(not_found)?;
    let run = Run::from(&doc).ok_or_else(not_found)?;
    if run.completed() {
        return Ok(run);
    }
    let doc = gh
        .json(&format!("/actions/runs/{id}"), None)
        .map_err(remote)?
        .ok_or_else(not_found)?;
    Run::from(&doc).ok_or_else(not_found)
}

fn gh_repo(gh: &Gh<'_>) -> String {
    gh.repo.to_string()
}

/// The latest run of each workflow on a PR's head that has failed or is
/// still going.
fn pr_runs(gh: &mut Gh<'_>, pr: u64) -> Result<(Value, Vec<Run>), Refusal> {
    let doc = gh
        .json(&format!("/pulls/{pr}"), None)
        .map_err(remote)?
        .ok_or_else(|| refusal(Exit::NotFound, format!("no PR #{pr} in {}", gh_repo(gh))))?;
    let sha = doc["head"]["sha"].as_str().unwrap_or_default().to_string();
    let list = gh
        .json(&format!("/actions/runs?head_sha={sha}&per_page=50"), None)
        .map_err(remote)?
        .unwrap_or_default();
    let mut latest: BTreeMap<u64, Run> = BTreeMap::new();
    for run in list["workflow_runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Run::from)
    {
        let newer = latest
            .get(&run.workflow_id)
            .is_none_or(|r| run.created_at > r.created_at);
        if newer {
            latest.insert(run.workflow_id, run);
        }
    }
    let runs = latest
        .into_values()
        .filter(|r| {
            !r.completed() || matches!(r.conclusion.as_str(), "failure" | "timed_out" | "cancelled")
        })
        .collect();
    Ok((doc, runs))
}

/// What a failed Entire Gates check comes down to, said once next to the
/// jobs rather than reported as another failure.
#[derive(Debug, Serialize)]
pub(crate) struct EntireNote {
    #[serde(flatten)]
    pub(crate) gates: Gates,
    /// The commit the check is on.
    pub(crate) head_sha: String,
    /// It only repeats failures reported elsewhere: failed checks, or a
    /// branch behind or conflicting with its base.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) derived: bool,
    pub(crate) note: String,
}

/// The Entire Gates check on PR `n`'s head, when it failed; `pr` is the
/// PR's document. Only a PR's current head is looked at: a run named on
/// its own may be on a head the PR has moved past, where an out-of-date
/// gate is noise.
fn entire_gates(gh: &mut Gh<'_>, n: u64, pr: &Value, failed_jobs: bool) -> Option<EntireNote> {
    let sha = pr["head"]["sha"].as_str().filter(|s| !s.is_empty())?;
    let doc = gh
        .json(
            &format!("/commits/{sha}/check-runs?check_name=Entire%20Gates&filter=latest"),
            Some(BRIEF),
        )
        .ok()??;
    let check = doc["check_runs"].as_array()?.first()?;
    let output = &check["output"];
    let gates = Gates::parse(
        check["status"].as_str().unwrap_or_default(),
        check["conclusion"].as_str(),
        output["title"].as_str(),
        output["summary"].as_str(),
        check["details_url"].as_str(),
    );
    if !gates.failed() {
        return None;
    }
    let base = pr["base"]["ref"].as_str().unwrap_or("BASE");
    let name = format!("{}#{n}", gh.repo);
    let mut parts = Vec::new();
    if gates.fails("checks") {
        parts.push(if failed_jobs {
            "it repeats the failed jobs above".to_string()
        } else {
            "it repeats failing checks outside the runs explained here".to_string()
        });
    }
    if let Some(g) = gates.failing().find(|g| g.gate == "up_to_date") {
        parts.push(format!(
            "the branch is not up to date with {base} ({}): git fetch origin {base} && git merge origin/{base}",
            g.reason.as_deref().unwrap_or("behind or conflicting")
        ));
    }
    if gates.fails("findings") {
        parts.push(format!(
            "it has open findings, which a rerun cannot fix: jouet pr threads {name}"
        ));
    }
    let note = parts.join("; ");
    Some(EntireNote {
        derived: gates.derived(),
        gates,
        head_sha: sha.to_string(),
        note,
    })
}

/// The jobs of a run's attempt; a finished attempt's never change.
fn jobs(gh: &mut Gh<'_>, run_id: u64, attempt: u64, done: bool) -> Result<Vec<Job>, Failure> {
    let keep = done.then_some(cache::MAX_AGE);
    let v = gh.json(
        &format!("/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100"),
        keep,
    )?;
    Ok(v.as_ref()
        .and_then(|v| v["jobs"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Job::from)
        .collect())
}

/// A failed job's log, cut to its failed step and read for a signature.
fn read(gh: &mut Gh<'_>, job: &Job, log_lines: usize) -> Read {
    let step = Step::failed(&job.doc);
    let step_name = step.as_ref().map(|s| s.name.clone());
    match gh.log(job.id) {
        Ok((log, path)) => {
            let whole = joblog::excerpt(&log, step.as_ref(), 0).lines;
            let sig = Signature::of(step_name.as_deref(), &whole);
            Read {
                job: job.clone(),
                excerpt: focus(whole, &sig, log_lines),
                sig,
                step: step_name,
                log_path: path.map(|p| p.display().to_string()),
                unread: None,
            }
        }
        Err(e) => Read {
            job: job.clone(),
            sig: Signature::of(step_name.as_deref(), &[]),
            excerpt: joblog::Excerpt {
                lines: Vec::new(),
                truncated: false,
            },
            step: step_name,
            log_path: None,
            unread: Some(format!("could not read its log: {}", e.message())),
        },
    }
}

/// A failed step's lines cut to `budget`: its title, the lines that name
/// the failure (a failed test, an error, the runner's complaint) with a
/// little context, and the end, with a marker wherever lines were left
/// out. `budget` 0 keeps them all.
fn focus(lines: Vec<String>, sig: &Signature, budget: usize) -> joblog::Excerpt {
    if budget == 0 || lines.len() <= budget {
        return joblog::Excerpt {
            lines,
            truncated: false,
        };
    }
    let mut needles: Vec<&str> = sig.tests.iter().map(|t| t.name.as_str()).collect();
    needles.extend(sig.errors.iter().map(String::as_str));
    needles.extend(sig.infra.iter().map(|i| i.line.as_str()));
    needles.extend(sig.location.as_deref());
    let mut keep = vec![false; lines.len()];
    let tail = budget / 2;
    for k in &mut keep[lines.len() - tail..] {
        *k = true;
    }
    let mut left = budget - tail;
    if left > 0 {
        keep[0] = true;
        left -= 1;
    }
    let head = lines.len() - tail;
    for i in (0..head).filter(|&i| needles.iter().any(|n| lines[i].contains(n))) {
        for k in &mut keep[i.saturating_sub(1)..(i + 3).min(head)] {
            if left > 0 && !*k {
                *k = true;
                left -= 1;
            }
        }
    }
    let mut out = Vec::new();
    let mut skipped = 0;
    for (line, kept) in lines.into_iter().zip(&keep) {
        if *kept {
            if skipped > 0 {
                out.push(skipped_marker(skipped));
                skipped = 0;
            }
            out.push(line);
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        out.push(skipped_marker(skipped));
    }
    joblog::Excerpt {
        lines: out,
        truncated: true,
    }
}

fn skipped_marker(n: usize) -> String {
    if n == 1 {
        "... 1 line skipped".into()
    } else {
        format!("... {n} lines skipped")
    }
}

/// The signature of a job in another run, through the history budget.
fn history_signature(gh: &mut Gh<'_>, history: &mut History, job: &Job) -> Option<Signature> {
    if history.logs_left == 0 {
        return None;
    }
    history.logs_left -= 1;
    let (log, _) = gh.log(job.id).ok()?;
    let step = Step::failed(&job.doc);
    let lines = joblog::excerpt(&log, step.as_ref(), 0).lines;
    Some(Signature::of(
        step.as_ref().map(|s| s.name.as_str()),
        &lines,
    ))
}

/// The jobs of another run, through the history budget.
fn history_jobs(
    gh: &mut Gh<'_>,
    history: &mut History,
    run: &Run,
    attempt: u64,
) -> Option<Vec<Job>> {
    if history.runs_left == 0 {
        return None;
    }
    history.runs_left -= 1;
    jobs(gh, run.id, attempt, run.completed()).ok()
}

fn explain_run(
    gh: &mut Gh<'_>,
    history: &mut History,
    run: &Run,
    only_job: Option<u64>,
    log_lines: usize,
) -> Result<RunReport, Refusal> {
    let jobs = jobs(gh, run.id, run.attempt, run.completed()).map_err(remote)?;
    let failed: Vec<&Job> = jobs.iter().filter(|j| j.failed()).collect();
    let mut reads: Vec<Read> = Vec::new();
    let mut unread: Vec<&Job> = Vec::new();
    for job in &failed {
        if reads.len() < OWN_LOGS {
            reads.push(read(gh, job, log_lines));
        } else {
            unread.push(job);
        }
    }
    // A job with nothing of its own to say that started once another had
    // failed is reporting that failure, as a `needs:` job with
    // `if: always()` does.
    let primary_end = reads
        .iter()
        .filter(|r| !r.sig.is_empty())
        .filter_map(|r| r.job.completed_at.map(|t| (t, r.job.name.clone())))
        .min();
    let mut following = Vec::new();
    reads.retain(|r| {
        let follows = r.sig.is_empty()
            && r.unread.is_none()
            && primary_end
                .as_ref()
                .is_some_and(|(end, _)| r.job.started_at.is_some_and(|s| s >= *end));
        if follows && let Some((_, name)) = &primary_end {
            following.push(Following {
                name: r.job.name.clone(),
                job_id: r.job.id,
                conclusion: r.job.conclusion.clone(),
                follows: name.clone(),
            });
        }
        !follows
    });
    let first_failed = reads.first().map(|r| r.job.name.clone());
    let cancelled: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.conclusion == "cancelled")
        .collect();
    match &first_failed {
        Some(name) => following.extend(cancelled.iter().map(|j| Following {
            name: j.name.clone(),
            job_id: j.id,
            conclusion: j.conclusion.clone(),
            follows: name.clone(),
        })),
        // Cancelled with nothing failed: by hand, by a newer run, or the
        // runner, which its log may say.
        None if run.completed() => {
            for job in cancelled.iter().take(OWN_LOGS) {
                reads.push(read(gh, job, log_lines));
            }
        }
        None => {}
    }
    if let Some(id) = only_job {
        reads.retain(|r| r.job.id == id);
        following.retain(|f| f.job_id == id);
        unread.retain(|j| j.id == id);
    }
    // Jobs that failed the same way are judged once.
    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for (i, r) in reads.iter().enumerate() {
        let key = r.sig.group_key();
        match groups.iter_mut().find(|(k, _)| key.is_some() && *k == key) {
            Some((_, members)) => members.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    let mut reports: Vec<Option<JobReport>> = (0..reads.len()).map(|_| None).collect();
    for (_, members) in &groups {
        let lead = &reads[members[0]];
        let facts = gather(gh, history, run, &jobs, lead);
        let mut judged = verdict::judge(&lead.sig, &facts);
        if let Some(why) = &lead.unread {
            judged.verdict = Verdict::Unknown;
            judged.evidence = vec![why.clone()];
        } else if lead.job.conclusion == "cancelled" && judged.verdict != Verdict::Infrastructure {
            judged.verdict = Verdict::Unknown;
            judged.evidence = vec![
                "cancelled, not after a failure in this run: by hand, by a newer run, or a runner its log does not name"
                    .into(),
            ];
        }
        for &i in members {
            reports[i] = Some(job_report(&reads[i], &judged, members[0]));
        }
    }
    let mut jobs_out: Vec<JobReport> = reports.into_iter().flatten().collect();
    for job in unread {
        jobs_out.push(JobReport {
            name: job.name.clone(),
            job_id: job.id,
            url: job.url.clone(),
            os: job.os,
            verdict: Verdict::Unknown,
            failed_step: Step::failed(&job.doc).map(|s| s.name),
            tests: Vec::new(),
            location: None,
            errors: Vec::new(),
            infrastructure: None,
            main_broken: false,
            evidence: vec![format!(
                "its log was not read: only the first {OWN_LOGS} failed jobs of a run are"
            )],
            group: jobs_out.len(),
            log_excerpt: Vec::new(),
            log_truncated: false,
            log_path: None,
        });
    }
    Ok(RunReport {
        run_id: run.id,
        workflow: run.workflow.clone(),
        head_sha: run.head_sha.clone(),
        head_branch: run.head_branch.clone(),
        attempt: run.attempt,
        url: run.url.clone(),
        prs: run.prs.clone(),
        in_progress: !run.completed(),
        jobs: jobs_out,
        following,
    })
}

fn job_report(r: &Read, judged: &verdict::Judged, group: usize) -> JobReport {
    JobReport {
        name: r.job.name.clone(),
        job_id: r.job.id,
        url: r.job.url.clone(),
        os: r.job.os,
        verdict: judged.verdict,
        failed_step: r.step.clone(),
        tests: r.sig.tests.iter().map(|t| t.name.clone()).collect(),
        location: r.sig.location.clone(),
        errors: r.sig.errors.clone(),
        infrastructure: r.sig.infrastructure().cloned(),
        main_broken: judged.main_broken,
        evidence: judged.evidence.clone(),
        group,
        log_excerpt: r.excerpt.lines.clone(),
        log_truncated: r.excerpt.truncated,
        log_path: r.log_path.clone(),
    }
}

impl History {
    fn main(&mut self, gh: &mut Gh<'_>) -> String {
        self.main
            .get_or_insert_with(|| gh.default_branch().unwrap_or_else(|_| "main".into()))
            .clone()
    }

    /// The workflow's recent runs, newest first.
    fn workflow_runs(&mut self, gh: &mut Gh<'_>, workflow_id: u64) -> Vec<Run> {
        self.workflow_runs
            .entry(workflow_id)
            .or_insert_with(|| {
                list(
                    gh,
                    &format!("/actions/workflows/{workflow_id}/runs?per_page={WORKFLOW_RUNS}"),
                )
            })
            .clone()
    }
}

fn list(gh: &mut Gh<'_>, path: &str) -> Vec<Run> {
    let mut runs: Vec<Run> = gh
        .json(path, Some(BRIEF))
        .ok()
        .flatten()
        .and_then(|v| v["workflow_runs"].as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|v| Run::from(&v))
        .collect();
    runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    runs
}

/// What history says about the lead job of a group of same failures.
fn gather(gh: &mut Gh<'_>, history: &mut History, run: &Run, jobs: &[Job], lead: &Read) -> Facts {
    let main = history.main(gh);
    let mut f = Facts {
        os: lead.job.os,
        on_main: run.head_branch == main,
        main_branch: main.clone(),
        workflow: run.workflow.clone(),
        ..Facts::default()
    };
    let sig = &lead.sig;
    if lead.unread.is_some() || sig.infrastructure().is_some() {
        return f;
    }
    if !sig.errors.is_empty() {
        if !f.on_main {
            main_errors(gh, history, run, &main, &lead.job.name, sig, &mut f);
        }
        return f;
    }
    let runs = history.workflow_runs(gh, run.workflow_id);
    same_commit(gh, history, run, lead, &mut f);
    if f.same_commit.iter().any(|s| s.passed || s.same) {
        return f;
    }
    if !f.on_main {
        main_now(gh, history, run, &runs, &main, lead, &mut f);
        if !f.main_failed.is_empty() {
            return f;
        }
    }
    if sig.tests.is_empty() {
        return f;
    }
    siblings(gh, history, jobs, lead, &mut f);
    if !f.passed_on_commit.is_empty() {
        return f;
    }
    elsewhere(gh, history, run, &runs, lead, &mut f);
    f
}

/// The job in another run that matches `name`.
fn same_job<'a>(jobs: &'a [Job], name: &str) -> Option<&'a Job> {
    jobs.iter().find(|j| j.name == name)
}

/// Earlier attempts of this run, and other runs of the workflow on the
/// same commit.
fn same_commit(gh: &mut Gh<'_>, history: &mut History, run: &Run, lead: &Read, f: &mut Facts) {
    let mut others: Vec<(Run, u64)> = (1..run.attempt).rev().map(|a| (run.clone(), a)).collect();
    let on_commit = list(
        gh,
        &format!(
            "/actions/workflows/{}/runs?head_sha={}&per_page={SAME_COMMIT_RUNS}",
            run.workflow_id, run.head_sha
        ),
    );
    others.extend(
        on_commit
            .into_iter()
            .filter(|r| r.id != run.id && r.completed())
            .map(|r| {
                let attempt = r.attempt;
                (r, attempt)
            }),
    );
    for (other, attempt) in others {
        let Some(jobs) = history_jobs(gh, history, &other, attempt) else {
            break;
        };
        let Some(job) = same_job(&jobs, &lead.job.name) else {
            continue;
        };
        let mut place = other.place();
        place.attempt = attempt;
        if job.passed() {
            f.same_commit.push(Seen {
                place,
                passed: true,
                same: false,
            });
            return;
        }
        if job.failed() {
            let same = history_signature(gh, history, job)
                .is_some_and(|s| s.same_as(&lead.sig) || (s.is_empty() && lead.sig.is_empty()));
            f.same_commit.push(Seen {
                place,
                passed: false,
                same,
            });
        }
    }
}

/// Whether the workflow's latest finished run on the default branch fails
/// this job the same way now.
fn main_now(
    gh: &mut Gh<'_>,
    history: &mut History,
    run: &Run,
    runs: &[Run],
    main: &str,
    lead: &Read,
    f: &mut Facts,
) {
    let Some(latest) = runs.iter().find(|r| {
        r.head_branch == main && r.completed() && r.id != run.id && r.conclusion != "cancelled"
    }) else {
        return;
    };
    f.looked_at += 1;
    if !latest.failed() {
        return;
    }
    let Some(jobs) = history_jobs(gh, history, latest, latest.attempt) else {
        return;
    };
    let Some(job) = same_job(&jobs, &lead.job.name).filter(|j| j.failed()) else {
        return;
    };
    let same_step = Step::failed(&job.doc).map(|s| s.name) == lead.step;
    let matched = history_signature(gh, history, job).is_some_and(|s| {
        s.same_as(&lead.sig) || (lead.sig.is_empty() && s.is_empty() && same_step)
    });
    if matched {
        f.main_failed.push(latest.place());
    }
}

/// For a compile error: the default branch's failed runs, in any workflow,
/// that failed with it too, and whether a run passed there since.
fn main_errors(
    gh: &mut Gh<'_>,
    history: &mut History,
    run: &Run,
    main: &str,
    name: &str,
    sig: &Signature,
    f: &mut Facts,
) {
    // The default branch as it was when this run started, whenever the
    // explain runs: its failures from a few hours before to just after.
    let at = jouet_output::time::parse_utc(&run.created_at).unwrap_or_else(jouet_output::time::now);
    let window = format!(
        "{}..{}",
        jouet_output::time::format_utc(at - MAIN_BEFORE),
        jouet_output::time::format_utc(at + MAIN_AFTER)
    );
    let failures = list(
        gh,
        &format!(
            "/actions/runs?branch={main}&status=failure&created={window}&per_page={MAIN_SCAN}"
        ),
    );
    for failed in failures.iter().take(MAIN_SCAN) {
        let Some(jobs) = history_jobs(gh, history, failed, failed.attempt) else {
            break;
        };
        let job = same_job(&jobs, name)
            .filter(|j| j.failed())
            .or_else(|| jobs.iter().find(|j| j.failed()));
        let Some(job) = job else {
            continue;
        };
        if history_signature(gh, history, job).is_some_and(|s| s.same_as(sig)) {
            f.main_failed.push(failed.place());
        }
    }
    // Whether the newest match's workflow has passed there since, now.
    let Some(newest) = f.main_failed.first() else {
        return;
    };
    let Some(matched) = failures.iter().find(|r| r.id == newest.run_id) else {
        return;
    };
    let passes = list(
        gh,
        &format!("/actions/runs?branch={main}&status=success&per_page={MAIN_RUNS}"),
    );
    f.main_green_since = passes
        .iter()
        .find(|r| r.workflow_id == matched.workflow_id && r.created_at > matched.created_at)
        .map(Run::place);
}

/// Jobs of this run that passed the failed tests, most like the failed
/// job's name first.
fn siblings(gh: &mut Gh<'_>, history: &mut History, jobs: &[Job], lead: &Read, f: &mut Facts) {
    let mut passed: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.passed() && j.id != lead.job.id)
        .collect();
    let common = |j: &Job| {
        j.name
            .chars()
            .zip(lead.job.name.chars())
            .take_while(|(a, b)| a == b)
            .count()
    };
    passed.sort_by_key(|j| std::cmp::Reverse(common(j)));
    for job in passed.into_iter().take(SIBLING_LOGS) {
        if history.logs_left == 0 {
            break;
        }
        history.logs_left -= 1;
        let Ok((log, _)) = gh.log(job.id) else {
            continue;
        };
        let lines = joblog::excerpt(&log, None, 0).lines;
        if lead.sig.tests.iter().any(|t| signature::passed(&lines, t)) {
            f.passed_on_commit.push((job.name.clone(), job.os));
        }
    }
}

/// Other commits whose run of this job failed the same way, and whether
/// their branch passed later.
fn elsewhere(
    gh: &mut Gh<'_>,
    history: &mut History,
    run: &Run,
    runs: &[Run],
    lead: &Read,
    f: &mut Facts,
) {
    for other in runs
        .iter()
        .filter(|r| r.failed() && r.head_sha != run.head_sha)
    {
        if history.runs_left == 0 || history.logs_left == 0 {
            break;
        }
        let Some(jobs) = history_jobs(gh, history, other, other.attempt) else {
            break;
        };
        f.looked_at += 1;
        let Some(job) = same_job(&jobs, &lead.job.name).filter(|j| j.failed()) else {
            continue;
        };
        if history_signature(gh, history, job).is_some_and(|s| s.same_as(&lead.sig)) {
            let green = runs.iter().any(|r| {
                r.head_branch == other.head_branch
                    && r.created_at > other.created_at
                    && r.conclusion == "success"
            });
            f.elsewhere.push((other.place(), green));
            if green {
                return;
            }
        }
    }
}

/// Whether `--rerun` may rerun the failed jobs, and the command that does.
fn rerun_plan(reports: &[RunReport], target: &str, in_progress: bool) -> Rerun {
    let failed: Vec<&JobReport> = reports.iter().flat_map(|r| &r.jobs).collect();
    let refuse = |reason: String| Rerun {
        allowed: false,
        reason,
        command: None,
        jobs: Vec::new(),
        started: false,
    };
    if failed.is_empty() {
        return refuse(if in_progress {
            "nothing has failed yet; the run is still in progress".into()
        } else {
            "nothing failed".into()
        });
    }
    let blocking: Vec<String> = failed
        .iter()
        .filter(|j| !j.verdict.rerunnable())
        .map(|j| format!("{} ({})", j.name, j.verdict.as_str()))
        .collect();
    if !blocking.is_empty() {
        let real = failed.iter().any(|j| j.verdict == Verdict::Real);
        let why = if real {
            "a rerun repeats a real failure"
        } else {
            "a failure without a diagnosis is not rerun blind"
        };
        return refuse(format!("{why}: {}", short_list(&blocking)));
    }
    if let Some(r) = reports.iter().find(|r| r.in_progress) {
        return refuse(format!(
            "run {} is still in progress, and GitHub reruns only finished runs (`jouet wait run {}`)",
            r.run_id, r.run_id
        ));
    }
    let command = if target.is_empty() {
        "jouet ci explain --rerun".to_string()
    } else {
        format!("jouet ci explain {target} --rerun")
    };
    Rerun {
        allowed: true,
        reason: "every failed job is flaky or infrastructure".into(),
        command: Some(command),
        jobs: failed.iter().map(|j| j.name.clone()).collect(),
        started: false,
    }
}

/// Up to three items, then how many more.
pub(crate) fn short_list(items: &[String]) -> String {
    if items.len() <= 3 {
        return verdict::join_and(items);
    }
    format!("{} and {} more", items[..3].join(", "), items.len() - 3)
}
