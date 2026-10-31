//! `wait run`: wait for one GitHub Actions run, failing fast on its first
//! failed job.

use super::checks::{self, Check, Counts, Verdict};
use super::ci::{attach_logs, result_exit};
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

/// Reads `RUN`: a run ID, or an Actions URL such as
/// `https://github.com/o/r/actions/runs/456/job/123`, which also names the
/// repository.
pub fn parse_run(arg: &str) -> Option<(Option<Repo>, u64)> {
    if let Ok(id) = arg.parse::<u64>() {
        return (id > 0).then_some((None, id));
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
            run_id: self.id,
            workflow: self.workflow.clone(),
            head_sha: self.head_sha.clone(),
            result,
            elapsed_s: self.start.elapsed().as_secs(),
            counts: Counts::of(&jobs),
            checks: jobs,
        }
    }

    fn fetch_jobs(&mut self, client: &mut Client) -> Result<Vec<Check>, Halt> {
        let mut jobs = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!(
                "/repos/{}/{}/actions/runs/{}/jobs?per_page=100&page={page}",
                self.repo.owner, self.repo.name, self.id
            );
            let Some(v) = watch::get_json(client, &mut self.cache, &path)? else {
                break;
            };
            let list = v["jobs"].as_array().cloned().unwrap_or_default();
            jobs.extend(list.iter().filter_map(Check::from_job));
            let total = v["total_count"].as_u64().unwrap_or(0);
            if list.len() < 100 || jobs.len() as u64 >= total {
                break;
            }
        }
        Ok(jobs)
    }
}

impl Watch for RunWait {
    type Doc = Doc;

    fn label(&self) -> String {
        format!("run {} {}", self.repo, self.id)
    }

    fn poll(
        &mut self,
        client: &mut Client,
        _interval: Duration,
        _events: &mut Vec<Event>,
    ) -> Result<Poll<Doc>, Halt> {
        let path = format!(
            "/repos/{}/{}/actions/runs/{}",
            self.repo.owner, self.repo.name, self.id
        );
        let run: Value = watch::get_json(client, &mut self.cache, &path)?.ok_or_else(|| {
            Halt::not_found(format!("run {} does not exist in {}", self.id, self.repo))
        })?;
        self.workflow = run["name"].as_str().map(str::to_string);
        self.head_sha = run["head_sha"].as_str().unwrap_or_default().to_string();
        let jobs = self.fetch_jobs(client)?;
        self.jobs = jobs.clone();
        let complete = run["status"].as_str() == Some("completed");
        let failed_run = complete
            && !matches!(
                run["conclusion"].as_str(),
                Some("success" | "skipped" | "neutral")
            );
        Ok(match (checks::verdict(&jobs, false), complete) {
            (Verdict::Failure, _) => {
                let mut jobs = jobs;
                if let Some(lines) = self.log_lines {
                    attach_logs(client, &self.repo, &mut jobs, lines);
                }
                if self.local_runner {
                    checks::suggest_local(&mut jobs, self.workflow.as_deref());
                }
                Poll::Done(self.doc("failure", jobs))
            }
            (_, true) if failed_run => Poll::Done(self.doc("failure", jobs)),
            (_, true) => Poll::Done(self.doc("success", jobs)),
            (_, false) if jobs.is_empty() => {
                Poll::Pending(run["status"].as_str().unwrap_or("queued").to_string())
            }
            (_, false) => Poll::Pending(Counts::of(&jobs).summary()),
        })
    }

    fn timed_out(&mut self, _client: &mut Client) -> Doc {
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
    }
}
