//! `wait run`: wait for one GitHub Actions run or GitLab pipeline, failing
//! fast on its first failed job.

use super::checks::{self, Check, Counts, Verdict};
use super::ci::result_exit;
use super::host::{Remote, Run};
use super::remote::{Halt, Poll, Watch};
use super::watch::{self, Cached, short};
use super::{Event, Reporter};
use jouet_github::Client;
use jouet_github::pr::target::Repo;
use jouet_output::exit::Exit;
use jouet_output::time::elapsed;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Pages of jobs read at most (100 each).
const MAX_PAGES: u32 = 10;

/// Reads `RUN`: a run ID, an Actions URL such as
/// `https://github.com/o/r/actions/runs/456/job/123`, or a GitLab pipeline
/// URL such as `https://gitlab.com/group/project/-/pipelines/456`; a URL
/// also names the repository.
pub fn parse_run(arg: &str) -> Option<(Option<Repo>, u64)> {
    if let Ok(id) = arg.parse::<u64>() {
        return (id > 0).then_some((None, id));
    }
    if let Some((_, rest)) = arg.split_once("://")
        && let Some((repo, after)) = rest.split_once("/-/pipelines/")
        && let Some((host, path)) = repo.split_once('/')
    {
        let id = after.split(['/', '?', '#']).next()?.parse().ok()?;
        return Some((Some(Repo::on(host, path)?), id));
    }
    let id = checks::run_id_in_url(arg)?;
    let (_, rest) = arg.split_once("://")?;
    let mut parts = rest.split('/').skip(1);
    let repo = Repo::parse(&format!("{}/{}", parts.next()?, parts.next()?))?;
    Some((Some(repo), id))
}

pub struct RunWait {
    repo: Repo,
    id: u64,
    log_lines: Option<usize>,
    cache: BTreeMap<String, Cached>,
    start: Instant,
    workflow: Option<String>,
    head_sha: String,
    jobs: Vec<Check>,
    local_runner: bool,
}

#[derive(Debug, Serialize)]
pub struct Doc {
    pub(crate) repo: String,
    /// The host of a repository that is not on GitHub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<String>,
    pub(crate) run_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workflow: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) head_sha: String,
    /// `success`, `failure` or `timeout`.
    pub(crate) result: &'static str,
    pub(crate) elapsed_s: u64,
    pub(crate) counts: Counts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) checks: Vec<Check>,
}

impl RunWait {
    pub fn new(repo: Repo, id: u64, log_lines: Option<usize>) -> Self {
        Self {
            repo,
            id,
            log_lines,
            cache: BTreeMap::new(),
            start: Instant::now(),
            workflow: None,
            head_sha: String::new(),
            jobs: Vec::new(),
            local_runner: false,
        }
    }

    /// Suggests `jouet ci run` for a failed job, for when jactionlint is
    /// installed.
    #[must_use]
    pub fn local_runner(mut self, on: bool) -> Self {
        self.local_runner = on;
        self
    }

    fn doc(&self, result: &'static str, mut jobs: Vec<Check>) -> Doc {
        jobs.sort_by_key(|j| std::cmp::Reverse(j.state.failed()));
        Doc {
            repo: self.repo.to_string(),
            host: self.repo.host.clone(),
            run_id: self.id,
            workflow: self.workflow.clone(),
            head_sha: self.head_sha.clone(),
            result,
            elapsed_s: self.start.elapsed().as_secs(),
            counts: Counts::of(&jobs),
            checks: jobs,
        }
    }
}

/// A GitHub Actions run and its jobs, through the ETag cache.
pub(crate) fn github_run(
    client: &mut Client,
    repo: &Repo,
    id: u64,
    cache: &mut BTreeMap<String, Cached>,
) -> Result<Run, Halt> {
    let base = format!("/repos/{}/{}/actions/runs/{id}", repo.owner, repo.name);
    let run: Value = watch::get_json(client, cache, &base)?
        .ok_or_else(|| Halt::not_found(format!("run {id} does not exist in {repo}")))?;
    let mut jobs = Vec::new();
    for page in 1..=MAX_PAGES {
        let path = format!("{base}/jobs?per_page=100&page={page}");
        let Some(v) = watch::get_json(client, cache, &path)? else {
            break;
        };
        let list = v["jobs"].as_array().cloned().unwrap_or_default();
        jobs.extend(list.iter().filter_map(Check::from_job));
        let total = v["total_count"].as_u64().unwrap_or(0);
        if list.len() < 100 || jobs.len() as u64 >= total {
            break;
        }
    }
    let complete = run["status"].as_str() == Some("completed");
    Ok(Run {
        workflow: run["name"].as_str().map(str::to_string),
        head_sha: run["head_sha"].as_str().unwrap_or_default().to_string(),
        status: run["status"].as_str().unwrap_or("queued").to_string(),
        failed: complete
            && !matches!(
                run["conclusion"].as_str(),
                Some("success" | "skipped" | "neutral")
            ),
        complete,
        jobs,
    })
}

impl Watch for RunWait {
    type Doc = Doc;

    fn label(&self) -> String {
        format!("run {} {}", self.repo, self.id)
    }

    fn poll(
        &mut self,
        remote: &mut Remote,
        _interval: Duration,
        _events: &mut Vec<Event>,
    ) -> Result<Poll<Doc>, Halt> {
        let run = remote.ci().run(&self.repo, self.id, &mut self.cache)?;
        self.workflow = run.workflow;
        self.head_sha = run.head_sha;
        let jobs = run.jobs;
        self.jobs = jobs.clone();
        Ok(match (checks::verdict(&jobs, false), run.complete) {
            (Verdict::Failure, _) => {
                let mut jobs = jobs;
                if let Some(lines) = self.log_lines {
                    remote.ci().attach_logs(&self.repo, &mut jobs, lines);
                }
                if self.local_runner && self.repo.host.is_none() {
                    checks::suggest_local(&mut jobs, self.workflow.as_deref());
                }
                Poll::Done(self.doc("failure", jobs))
            }
            (_, true) if run.failed => Poll::Done(self.doc("failure", jobs)),
            (_, true) => Poll::Done(self.doc("success", jobs)),
            (_, false) if jobs.is_empty() => Poll::Pending(run.status),
            (_, false) => Poll::Pending(Counts::of(&jobs).summary()),
        })
    }

    fn timed_out(&mut self, _remote: &mut Remote) -> Doc {
        self.doc("timeout", self.jobs.clone())
    }

    fn text(&self, doc: &Doc, reporter: &Reporter) -> String {
        let (word, good) = match doc.result {
            "success" => ("PASS", true),
            "failure" => ("FAIL", false),
            _ => ("PENDING", false),
        };
        let workflow = doc
            .workflow
            .as_deref()
            .map_or(String::new(), |w| format!(" ({w})"));
        let mut out = format!(
            "{} {} run {}{workflow} @ {}  {}  ({})\n",
            reporter.word(word, good),
            doc.repo,
            doc.run_id,
            short(&doc.head_sha),
            doc.counts.summary(),
            elapsed(doc.elapsed_s)
        );
        if doc.result == "timeout" {
            out.push_str("  still pending; run it again to keep waiting\n");
        }
        out.push_str(&checks::failure_blocks(&doc.checks));
        out
    }

    fn exit(&self, doc: &Doc) -> Exit {
        result_exit(doc.result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_are_ids_or_actions_urls() {
        assert_eq!(parse_run("456"), Some((None, 456)));
        let (repo, id) = parse_run("https://github.com/jdx/mise/actions/runs/456/job/123").unwrap();
        assert_eq!((repo.unwrap().to_string(), id), ("jdx/mise".into(), 456));
        assert_eq!(parse_run("0"), None);
        assert_eq!(parse_run("https://github.com/jdx/mise/pull/1"), None);
        let (repo, id) =
            parse_run("https://gitlab.com/group/sub/project/-/pipelines/77/builds").unwrap();
        assert_eq!(repo.unwrap().qualified(), "gitlab.com/group/sub/project");
        assert_eq!(id, 77);
    }
}
