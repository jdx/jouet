//! The GitHub reads `ci explain` makes, through its cache, counted.

use super::cache::{self, Cache};
use jouet_github::pr::target::Repo;
use jouet_github::{Client, Failure, Method};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// How long an answer that can change is reused: a run's document, a list
/// of runs.
pub(crate) const BRIEF: Duration = Duration::from_secs(5 * 60);
/// How long a repository's default branch is reused.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

pub(crate) struct Gh<'a> {
    client: &'a mut Client,
    pub(crate) repo: Repo,
    cache: Cache,
    /// Requests sent, and answers read from the cache.
    pub(crate) fetched: u32,
    pub(crate) cached: u32,
}

impl<'a> Gh<'a> {
    pub(crate) fn new(client: &'a mut Client, repo: Repo, cache_root: Option<PathBuf>) -> Self {
        let cache = Cache::new(cache_root, &repo.owner, &repo.name);
        Self {
            client,
            repo,
            cache,
            fetched: 0,
            cached: 0,
        }
    }

    fn base(&self) -> String {
        format!("/repos/{}/{}", self.repo.owner, self.repo.name)
    }

    /// GETs `path` (after `/repos/OWNER/NAME`), reusing an answer younger
    /// than `keep`; `None` keeps nothing, for an answer that is still
    /// changing. A 404 is `Ok(None)`.
    pub(crate) fn json(
        &mut self,
        path: &str,
        keep: Option<Duration>,
    ) -> Result<Option<Value>, Failure> {
        let full = format!("{}{path}", self.base());
        let key = cache::key(&full);
        if let Some(keep) = keep
            && let Some(text) = self.cache.read(&key, keep)
            && let Ok(v) = serde_json::from_str(&text)
        {
            self.cached += 1;
            return Ok(Some(v));
        }
        self.fetched += 1;
        let resp = self.client.get(&full, None)?;
        match resp.status {
            200 => {
                let v: Value = serde_json::from_str(&resp.body).map_err(|e| {
                    Failure::Other(format!(
                        "GitHub sent {full} in a form jouet could not read: {e}"
                    ))
                })?;
                if keep.is_some() {
                    self.cache.write(&key, &resp.body);
                }
                Ok(Some(v))
            }
            404 => Ok(None),
            status => Err(Failure::Other(format!(
                "GitHub returned {status} for {full}: {}",
                message(&resp.body)
            ))),
        }
    }

    /// The repository's default branch.
    pub(crate) fn default_branch(&mut self) -> Result<String, Failure> {
        let repo = self.json("", Some(DAY))?;
        Ok(repo
            .as_ref()
            .and_then(|r| r["default_branch"].as_str())
            .unwrap_or("main")
            .to_string())
    }

    /// A finished job's log, and the file it is kept in. A log `wait ci`
    /// already saved is read from there.
    pub(crate) fn log(&mut self, job_id: u64) -> Result<(String, Option<PathBuf>), Failure> {
        let saved = jouet_output::paths::log_dir().join(format!(
            "{}-{}-job-{job_id}.log",
            self.repo.owner, self.repo.name
        ));
        if let Ok(log) = std::fs::read_to_string(&saved) {
            self.cached += 1;
            return Ok((log, Some(saved)));
        }
        let key = format!("job-{job_id}.log");
        if let Some(log) = self.cache.read(&key, cache::MAX_AGE) {
            self.cached += 1;
            return Ok((log, self.cache.path(&key)));
        }
        self.fetched += 1;
        let path = format!("{}/actions/jobs/{job_id}/logs", self.base());
        let resp = self.client.get(&path, None)?;
        if resp.status != 200 {
            return Err(Failure::Other(format!(
                "GitHub returned {} for the log of job {job_id}: {}",
                resp.status,
                message(&resp.body)
            )));
        }
        self.cache.write(&key, &resp.body);
        let kept = self.cache.path(&key).filter(|p| p.exists());
        Ok((resp.body, kept))
    }

    /// POSTs to a rerun endpoint, and forgets the run's cached document,
    /// which the rerun makes stale.
    pub(crate) fn rerun(&mut self, path: &str, run_id: u64) -> Result<(), Failure> {
        self.fetched += 1;
        let full = format!("{}{path}", self.base());
        let resp = self
            .client
            .write(Method::Post, &full, &serde_json::json!({}))?;
        self.cache.forget(&cache::key(&format!(
            "{}/actions/runs/{run_id}",
            self.base()
        )));
        match resp.status {
            200..=299 => Ok(()),
            status => Err(Failure::Other(format!(
                "GitHub refused the rerun ({status}): {}",
                message(&resp.body)
            ))),
        }
    }
}

/// The `message` of a GitHub error body, or the start of the body.
fn message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.trim().chars().take(200).collect())
}
