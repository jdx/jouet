//! The forge a remote wait polls: GitHub, GitLab or Forgejo, each reading
//! its own API into the one model `wait ci`, `wait run` and `wait pr`
//! judge, so the three waits report the same way on every forge.
//!
//! GitHub's checks are check runs and commit statuses, read through ETags
//! with required checks and base-branch events; GitLab's are the commit
//! statuses of the head, which hold its pipelines' jobs (a job allowed to
//! fail counts as skipped, every other job as required); Forgejo's are the
//! head's commit statuses, which is where Forgejo Actions reports. Job logs
//! come from GitHub Actions and GitLab; Forgejo's API has none to give.
//! An Entire-hosted trail's checks are its check runs and its blocking
//! gates as one Entire Gates check, read through the `entire` CLI, which
//! gives no logs either.

use super::checks::{Check, State};
use super::ci;
use super::joblog;
use super::remote::Halt;
use super::run;
use super::watch::{self, Cached, PollError, Pr, Snapshot};
use jouet_github::entire_forge::{self, EntireForge};
use jouet_github::forgejo::{self, Forgejo};
use jouet_github::gitlab::{self, GitLab};
use jouet_github::host::Host;
use jouet_github::pr::target::{Repo, Target};
use jouet_github::{Client, Failure};
use serde_json::Value;
use std::collections::BTreeMap;

/// The forge a wait talks to.
pub enum Remote {
    GitHub(Client),
    GitLab(GitLab),
    Forgejo(Forgejo),
    Entire(EntireForge),
}

impl Remote {
    /// The forge `repo` is on, with its usual token sources.
    pub fn connect(repo: &Repo) -> Result<Self, String> {
        use jouet_github::forge::Forge;
        match (repo.forge(), repo.host.as_deref()) {
            (Some(Forge::GitLab), Some(host)) => Ok(Self::GitLab(GitLab::new(host))),
            (Some(Forge::Forgejo), Some(host)) => Ok(Self::Forgejo(Forgejo::new(host))),
            (Some(Forge::Entire), Some(_)) => Ok(Self::Entire(EntireForge::new())),
            (None, Some(host)) => Err(jouet_github::forge::unknown(host)),
            _ => Ok(Self::GitHub(Client::from_env())),
        }
    }

    /// The PR-level host, for status and attention.
    pub fn host(&mut self) -> &mut dyn Host {
        match self {
            Self::GitHub(c) => c,
            Self::GitLab(g) => g,
            Self::Forgejo(f) => f,
            Self::Entire(e) => e,
        }
    }

    /// The PR a target names: a number as is, a branch through its open
    /// PR, else its newest.
    pub fn resolve(&mut self, target: Target) -> Result<(Repo, u64), Halt> {
        self.ci().resolve(target)
    }

    pub(crate) fn ci(&mut self) -> &mut dyn Ci {
        match self {
            Self::GitHub(c) => c,
            Self::GitLab(g) => g,
            Self::Forgejo(f) => f,
            Self::Entire(e) => e,
        }
    }
}

/// One CI run, as `wait run` judges it.
pub(crate) struct Run {
    pub(crate) workflow: Option<String>,
    pub(crate) head_sha: String,
    /// The run's own status word, shown while it has no jobs.
    pub(crate) status: String,
    pub(crate) complete: bool,
    /// Finished, and not with success.
    pub(crate) failed: bool,
    pub(crate) jobs: Vec<Check>,
}

/// What a wait reads of CI on one forge.
pub(crate) trait Ci {
    /// The PR a target names: a number as is, a branch through its open PR,
    /// else its newest.
    fn resolve(&mut self, target: Target) -> Result<(Repo, u64), Halt>;
    /// Brings `snap` up to date: the PR, the commit followed (its head, or
    /// `pinned`), and that commit's checks, logging what changed.
    fn poll(
        &mut self,
        repo: &Repo,
        number: u64,
        pinned: Option<&str>,
        snap: &mut Snapshot,
    ) -> Result<(), PollError>;
    /// Fills in the log excerpt of each failed job whose log can be read.
    fn attach_logs(&mut self, repo: &Repo, checks: &mut [Check], lines: usize);
    /// A run and its jobs.
    fn run(
        &mut self,
        repo: &Repo,
        id: u64,
        cache: &mut BTreeMap<String, Cached>,
    ) -> Result<Run, Halt>;
}

impl Ci for Client {
    fn resolve(&mut self, target: Target) -> Result<(Repo, u64), Halt> {
        ci::resolve(self, target)
    }

    fn poll(
        &mut self,
        repo: &Repo,
        number: u64,
        pinned: Option<&str>,
        snap: &mut Snapshot,
    ) -> Result<(), PollError> {
        watch::poll(self, repo, number, pinned, snap)
    }

    fn attach_logs(&mut self, repo: &Repo, checks: &mut [Check], lines: usize) {
        ci::attach_logs(self, repo, checks, lines);
    }

    fn run(
        &mut self,
        repo: &Repo,
        id: u64,
        cache: &mut BTreeMap<String, Cached>,
    ) -> Result<Run, Halt> {
        run::github_run(self, repo, id, cache)
    }
}

fn not_found(target: &Target) -> Halt {
    Halt::not_found(format!("no PR for {target}"))
}

impl Ci for GitLab {
    fn resolve(&mut self, target: Target) -> Result<(Repo, u64), Halt> {
        match self.number(&target)? {
            Some(n) => Ok((target.repo().clone(), n)),
            None => Err(not_found(&target)),
        }
    }

    fn poll(
        &mut self,
        repo: &Repo,
        number: u64,
        pinned: Option<&str>,
        snap: &mut Snapshot,
    ) -> Result<(), PollError> {
        let path = format!(
            "{}/merge_requests/{number}?with_merge_status_recheck=true",
            gitlab::project(repo)
        );
        let mr = self
            .rest()
            .json(&path)?
            .ok_or_else(|| PollError::NotFound(format!("{repo}!{number} does not exist")))?;
        let detailed = mr["detailed_merge_status"].as_str().unwrap_or("unchecked");
        let conflicted = mr["has_conflicts"].as_bool() == Some(true)
            || matches!(detailed, "conflict" | "broken_status");
        let checking = matches!(detailed, "checking" | "unchecked" | "preparing");
        let pr = Pr {
            number,
            state: match mr["state"].as_str() {
                Some("merged") => "merged",
                Some("opened") => "open",
                _ => "closed",
            }
            .into(),
            head_sha: mr["sha"].as_str().unwrap_or_default().to_string(),
            base_ref: mr["target_branch"].as_str().unwrap_or_default().to_string(),
            mergeable: (!checking).then_some(!conflicted),
            mergeable_state: if conflicted { "dirty" } else { "clean" }.into(),
        };
        let sha = pinned.map_or_else(|| pr.head_sha.clone(), str::to_string);
        let checks = gitlab::statuses(self.rest(), repo, &sha)?
            .iter()
            .filter_map(gitlab_check)
            .collect();
        watch::record(snap, pr, sha, checks);
        Ok(())
    }

    fn attach_logs(&mut self, repo: &Repo, checks: &mut [Check], lines: usize) {
        for check in checks.iter_mut().filter(|c| c.state.failed()) {
            let Some(job_id) = check.job_id else {
                continue;
            };
            if let Err(e) = gitlab_log(self, repo, check, job_id, lines) {
                eprintln!(
                    "jouet: could not fetch the log of {} (job {job_id}): {}",
                    check.name,
                    e.message()
                );
            }
        }
    }

    fn run(
        &mut self,
        repo: &Repo,
        id: u64,
        _cache: &mut BTreeMap<String, Cached>,
    ) -> Result<Run, Halt> {
        let base = gitlab::project(repo);
        let pipeline = self
            .rest()
            .json(&format!("{base}/pipelines/{id}"))?
            .ok_or_else(|| Halt::not_found(format!("pipeline {id} does not exist in {repo}")))?;
        let jobs = self
            .rest()
            .list(&format!("{base}/pipelines/{id}/jobs?per_page=100"))?
            .unwrap_or_default();
        let status = pipeline["status"].as_str().unwrap_or("created").to_string();
        let complete = matches!(
            status.as_str(),
            "success" | "failed" | "canceled" | "skipped"
        );
        Ok(Run {
            workflow: pipeline["name"]
                .as_str()
                .or(pipeline["ref"].as_str())
                .map(str::to_string),
            head_sha: pipeline["sha"].as_str().unwrap_or_default().to_string(),
            failed: complete && !matches!(status.as_str(), "success" | "skipped"),
            complete,
            status,
            jobs: jobs
                .iter()
                .filter_map(|j| {
                    let mut check = gitlab_check(j)?;
                    check.job_id = j["id"].as_u64();
                    check.url = j["web_url"].as_str().map(str::to_string);
                    check.workflow = j["stage"].as_str().map(str::to_string);
                    check.run_id = Some(id);
                    Some(check)
                })
                .collect(),
        })
    }
}

/// A GitLab commit status or job. A job allowed to fail counts as skipped,
/// and every other job as required, as GitLab's pipeline treats them.
fn gitlab_check(v: &Value) -> Option<Check> {
    let status = v["status"].as_str().unwrap_or_default();
    let allowed = v["allow_failure"].as_bool() == Some(true);
    let state = match status {
        "success" => State::Pass,
        "failed" if allowed => State::Skipped,
        "failed" => State::Fail,
        "canceled" | "canceling" => State::Cancelled,
        "skipped" | "manual" => State::Skipped,
        _ => State::Pending,
    };
    let mut check = Check::new(v["name"].as_str()?.to_string(), state);
    if state.finished() {
        check.conclusion = Some(status.to_string());
    }
    check.required = !allowed;
    let url = v["target_url"].as_str().or(v["web_url"].as_str());
    check.url = url.map(str::to_string);
    // A job's commit status shares the job's ID and links to it.
    if url.is_some_and(|u| u.contains("/-/jobs/")) {
        check.job_id = v["id"].as_u64();
        check.run_id = v["pipeline_id"].as_u64();
    }
    Some(check)
}

fn gitlab_log(
    gl: &mut GitLab,
    repo: &Repo,
    check: &mut Check,
    job_id: u64,
    lines: usize,
) -> Result<(), Failure> {
    let dir = jouet_output::paths::log_dir();
    let path = dir.join(format!("{}-job-{job_id}.log", repo.slug()));
    let log = match std::fs::read_to_string(&path) {
        Ok(log) => log,
        Err(_) => {
            let url = format!("{}/jobs/{job_id}/trace", gitlab::project(repo));
            let resp = gl.rest().get(&url, None)?;
            if resp.status != 200 {
                return Err(gl.rest().refused(resp.status, &resp.body));
            }
            let log = gitlab_trace(&resp.body);
            std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&path, &log))
                .map_err(|e| Failure::Other(format!("saving {}: {e}", path.display())))?;
            log
        }
    };
    let excerpt = joblog::excerpt(&log, None, lines);
    check.log_excerpt = excerpt.lines;
    check.log_truncated = excerpt.truncated;
    check.log_path = Some(path.display().to_string());
    Ok(())
}

/// A GitLab job trace without its section markers, which open and close the
/// collapsible parts of the web view.
fn gitlab_trace(trace: &str) -> String {
    let mut out = String::new();
    for line in trace.lines() {
        let line = if line.starts_with("section_start:") || line.starts_with("section_end:") {
            line.split_once('\r').map_or("", |(_, rest)| rest)
        } else {
            line
        };
        let line = line.trim_start_matches("\u{1b}[0K");
        // A line of escapes alone, such as the colour reset after an error;
        // a line of digits is output.
        let bare = line.is_empty()
            || line.contains('\u{1b}')
                && line.chars().all(|c| {
                    c == '\u{1b}'
                        || c == '['
                        || c == ';'
                        || c.is_ascii_digit()
                        || c == 'm'
                        || c == 'K'
                });
        if !bare {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

impl Ci for Forgejo {
    fn resolve(&mut self, target: Target) -> Result<(Repo, u64), Halt> {
        match self.number(&target)? {
            Some(n) => Ok((target.repo().clone(), n)),
            None => Err(not_found(&target)),
        }
    }

    fn poll(
        &mut self,
        repo: &Repo,
        number: u64,
        pinned: Option<&str>,
        snap: &mut Snapshot,
    ) -> Result<(), PollError> {
        let path = format!("{}/pulls/{number}", forgejo::base(repo));
        let raw = self
            .rest()
            .json(&path)?
            .ok_or_else(|| PollError::NotFound(format!("{repo}#{number} does not exist")))?;
        let mergeable = raw["mergeable"].as_bool();
        let pr = Pr {
            number,
            state: if raw["merged"].as_bool() == Some(true) {
                "merged".into()
            } else {
                raw["state"].as_str().unwrap_or("open").to_string()
            },
            head_sha: raw["head"]["sha"].as_str().unwrap_or_default().to_string(),
            base_ref: raw["base"]["ref"].as_str().unwrap_or_default().to_string(),
            mergeable,
            mergeable_state: if mergeable == Some(false) {
                "dirty"
            } else {
                "clean"
            }
            .into(),
        };
        let sha = pinned.map_or_else(|| pr.head_sha.clone(), str::to_string);
        let checks = forgejo::statuses(self.rest(), repo, &sha)?
            .iter()
            .filter_map(|s| {
                let state = s["status"].as_str()?;
                let mut check = Check::new(
                    s["context"].as_str()?.to_string(),
                    match state {
                        "warning" | "skipped" => State::Skipped,
                        other => State::of_status(other),
                    },
                );
                if check.state.finished() {
                    check.conclusion = Some(state.to_string());
                }
                check.url = s["target_url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .map(|u| forgejo::link(self.rest(), u));
                Some(check)
            })
            .collect();
        watch::record(snap, pr, sha, checks);
        Ok(())
    }

    /// Forgejo's API gives no job logs; the checks keep their URLs.
    fn attach_logs(&mut self, _repo: &Repo, _checks: &mut [Check], _lines: usize) {}

    fn run(
        &mut self,
        _repo: &Repo,
        _id: u64,
        _cache: &mut BTreeMap<String, Cached>,
    ) -> Result<Run, Halt> {
        use jouet_github::host::Feature;
        let reason = self
            .lacks(Feature::Runs)
            .unwrap_or_else(|| "Forgejo runs cannot be read".into());
        Err(Halt::refused(reason))
    }
}

impl Ci for EntireForge {
    fn resolve(&mut self, target: Target) -> Result<(Repo, u64), Halt> {
        let doc = self.show(&target).map_err(Failure::Other)?;
        match doc.and_then(|d| d["number"].as_u64()) {
            Some(n) => Ok((target.repo().clone(), n)),
            None => Err(not_found(&target)),
        }
    }

    fn poll(
        &mut self,
        repo: &Repo,
        number: u64,
        pinned: Option<&str>,
        snap: &mut Snapshot,
    ) -> Result<(), PollError> {
        let doc = self
            .fresh(repo, number)
            .map_err(Failure::Other)?
            .ok_or_else(|| PollError::NotFound(format!("trail {repo}#{number} does not exist")))?;
        let (pr, checks) = entire_snapshot(&doc, number);
        // A trail's checks are its head's; a pinned commit that is no
        // longer the head has none to read.
        let sha = pinned.map_or_else(|| pr.head_sha.clone(), str::to_string);
        let checks = if sha == pr.head_sha {
            checks
        } else {
            Vec::new()
        };
        watch::record(snap, pr, sha, checks);
        Ok(())
    }

    /// Entire gives no job logs; the checks keep their URLs.
    fn attach_logs(&mut self, _repo: &Repo, _checks: &mut [Check], _lines: usize) {}

    fn run(
        &mut self,
        _repo: &Repo,
        _id: u64,
        _cache: &mut BTreeMap<String, Cached>,
    ) -> Result<Run, Halt> {
        use jouet_github::host::Feature;
        let reason = self
            .lacks(Feature::Runs)
            .unwrap_or_else(|| "Entire runs cannot be read".into());
        Err(Halt::refused(reason))
    }
}

/// The trail as a wait sees it: its state and head, and its check runs and
/// gates as checks.
fn entire_snapshot(doc: &Value, number: u64) -> (Pr, Vec<Check>) {
    let m = &doc["mergeability"];
    let conflicting = m["conflict_status"] == "conflicting";
    let pr = Pr {
        number,
        state: match doc["status"].as_str() {
            Some("merged") => "merged",
            Some("closed") => "closed",
            _ => "open",
        }
        .into(),
        head_sha: m["head_sha"].as_str().unwrap_or_default().to_string(),
        base_ref: doc["base"].as_str().unwrap_or_default().to_string(),
        mergeable: match m["conflict_status"].as_str() {
            Some("clean" | "conflicting") => Some(!conflicting),
            _ => None,
        },
        mergeable_state: if conflicting { "dirty" } else { "clean" }.into(),
    };
    let mut checks: Vec<Check> = entire_forge::runs(doc)
        .iter()
        .filter_map(Check::from_check_run)
        .collect();
    if let Some(gates) = entire_forge::gates(doc) {
        let state = match gates.state.as_str() {
            "passed" => State::Pass,
            "failed" => State::Fail,
            _ => State::Pending,
        };
        let mut check = Check::new(jouet_github::entire::CHECK.into(), state);
        if state.finished() {
            check.conclusion = Some(gates.state.clone());
        }
        check.url.clone_from(&gates.url);
        check.entire = Some(gates);
        checks.push(check);
    }
    (pr, checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gitlab_jobs_become_checks() {
        let failed = gitlab_check(&json!({
            "id": 9, "name": "test", "status": "failed", "pipeline_id": 4,
            "target_url": "https://gitlab.com/g/p/-/jobs/9", "allow_failure": false
        }))
        .unwrap();
        assert_eq!(failed.state, State::Fail);
        assert!(failed.required);
        assert_eq!((failed.job_id, failed.run_id), (Some(9), Some(4)));
        let allowed = gitlab_check(&json!({
            "id": 10, "name": "lint", "status": "failed", "allow_failure": true
        }))
        .unwrap();
        assert_eq!(allowed.state, State::Skipped);
        assert!(!allowed.required);
        // An external status has no job, so no log.
        let external = gitlab_check(&json!({
            "id": 11, "name": "deploy", "status": "running", "target_url": "https://ci.example.com/1"
        }))
        .unwrap();
        assert_eq!(external.job_id, None);
    }

    #[test]
    fn gitlab_traces_lose_their_section_markers() {
        let trace = "section_start:1:prepare\r\u{1b}[0K\u{1b}[0K\u{1b}[36;1mPreparing\u{1b}[0;m\n$ cargo test\n404\nerror: boom\nsection_end:2:step_script\r\u{1b}[0K\n\u{1b}[0;m\n";
        assert_eq!(
            gitlab_trace(trace),
            "\u{1b}[36;1mPreparing\u{1b}[0;m\n$ cargo test\n404\nerror: boom\n"
        );
    }
}
