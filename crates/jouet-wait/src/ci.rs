//! `wait ci`: wait for a PR's checks, fail fast, and show why they failed.

use super::checks::{self, Check, Counts, Selection, Verdict};
use super::joblog::{self, Step};
use super::remote::{Halt, Poll, Watch};
use super::watch::{self, PollError, Snapshot, short};
use super::{Event, Reporter, Stop};
use jouet_github::pr::target::{Repo, Target};
use jouet_github::{Client, Failure};
use jouet_output::exit::Exit;
use jouet_output::time::elapsed;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Options {
    pub selection: Selection,
    pub all: bool,
    /// Lines of each failure log to show; `None` fetches no logs.
    pub log_lines: Option<usize>,
    pub appear_timeout: Duration,
    /// How long a reported conflict must persist to be believed;
    /// [`crate::CONFLICT_CONFIRM`] except in tests.
    pub confirm_conflict: Duration,
    /// Whether jactionlint is installed, so a failed Actions job can be
    /// run here with `jouet ci run`.
    pub local_runner: bool,
}

pub struct CiWait {
    repo: Repo,
    number: u64,
    pinned: Option<String>,
    options: Options,
    state_path: PathBuf,
    start: Instant,
    /// Events up to this one were logged before this wait first looked, so
    /// they are replayed.
    replay_until: Option<u64>,
    /// The last event this wait has reported.
    seen: u64,
    /// The head when the wait began, and the one it is following now.
    first_sha: Option<String>,
    sha: Option<String>,
    /// When checks started being awaited on the current commit.
    sha_since: Instant,
    last: Snapshot,
    conflict: watch::ConflictCheck,
}

#[derive(Debug, Serialize)]
pub struct Doc {
    pub(crate) repo: String,
    pub(crate) pr: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) head_sha: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) head_changed: bool,
    /// `success`, `failure`, `timeout`, `conflict` or `no_checks`.
    pub(crate) result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    pub(crate) elapsed_s: u64,
    pub(crate) counts: Counts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) checks: Vec<Check>,
}

/// Finds the PR a target names: a number as is, a branch through its open
/// PR, else its newest.
pub fn resolve(client: &mut Client, target: Target) -> Result<(Repo, u64), Halt> {
    let (repo, branch) = match target {
        Target::Number(repo, n) => return Ok((repo, n)),
        Target::Branch(repo, branch) => (repo, branch),
    };
    for state in ["open", "all"] {
        let path = format!(
            "/repos/{}/{}/pulls?head={}:{}&state={state}&per_page=1",
            repo.owner, repo.name, repo.owner, branch
        );
        let mut cache = BTreeMap::new();
        if let Some(Value::Array(prs)) = watch::get_json(client, &mut cache, &path)?
            && let Some(n) = prs.first().and_then(|p| p["number"].as_u64())
        {
            return Ok((repo, n));
        }
    }
    Err(Halt::not_found(format!("no PR for {repo}:{branch}")))
}

impl CiWait {
    pub fn new(repo: Repo, number: u64, pinned: Option<String>, options: Options) -> Self {
        let state_path = watch::state_path(&repo, number, pinned.as_deref());
        let conflict = watch::ConflictCheck::new(options.confirm_conflict);
        Self {
            repo,
            number,
            pinned,
            options,
            state_path,
            start: Instant::now(),
            replay_until: None,
            seen: 0,
            first_sha: None,
            sha: None,
            sha_since: Instant::now(),
            last: Snapshot::default(),
            conflict,
        }
    }

    /// The state, polled from GitHub unless another wait polled it within
    /// `interval`.
    fn snapshot(&mut self, client: &mut Client, interval: Duration) -> Result<Snapshot, Halt> {
        let (repo, number, pinned) = (&self.repo, self.number, self.pinned.as_deref());
        let fresh_ms = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
        let replay_until = &mut self.replay_until;
        let polled = watch::with_state(&self.state_path, |snap| {
            replay_until.get_or_insert(snap.seq);
            let now_ms = watch::now_ms();
            if snap.pr.is_some() && now_ms.saturating_sub(snap.polled_at_ms) < fresh_ms {
                return Ok(snap.clone());
            }
            if let Some(reset) = snap.rate_limited_until
                && reset > jouet_output::time::now()
            {
                return Err(Halt::RateLimited { reset });
            }
            snap.rate_limited_until = None;
            match watch::poll(client, repo, number, pinned, snap) {
                Ok(()) => {
                    snap.polled_at_ms = now_ms;
                    Ok(snap.clone())
                }
                Err(PollError::Remote(Failure::RateLimited { reset })) => {
                    snap.rate_limited_until = Some(reset);
                    Err(Halt::RateLimited { reset })
                }
                Err(PollError::Remote(f)) => Err(f.into()),
                Err(PollError::NotFound(message)) => Err(Halt::not_found(message)),
            }
        });
        polled.map_err(|message| {
            Halt::Stop(Stop {
                exit: Exit::Failed,
                message,
            })
        })?
    }

    /// Events from the state log this wait has not reported yet. Ones from
    /// before it started are replayed, and say so.
    fn new_events(&mut self, snap: &Snapshot, events: &mut Vec<Event>) {
        let seen = self.seen;
        for logged in snap.events.iter().filter(|e| e.seq > seen) {
            self.seen = logged.seq;
            let name = logged.fields["name"].as_str();
            if name.is_some_and(|n| !self.options.selection.selects_name(n)) {
                continue;
            }
            let Some(kind) = event_name(&logged.name) else {
                continue;
            };
            let mut fields = logged.fields.clone();
            if self.replay_until.is_some_and(|until| logged.seq <= until)
                && let Some(map) = fields.as_object_mut()
            {
                map.insert("replayed".into(), json!(true));
            }
            events.push(Event {
                name: kind,
                fields,
                text: logged.text.clone(),
            });
        }
    }

    fn doc(&self, result: &'static str, mut checks: Vec<Check>, message: Option<String>) -> Doc {
        checks.sort_by(|a, b| {
            b.state
                .failed()
                .cmp(&a.state.failed())
                .then_with(|| a.name.cmp(&b.name))
        });
        let sha = self.sha.clone().unwrap_or_default();
        Doc {
            repo: self.repo.to_string(),
            pr: self.number,
            head_changed: self.first_sha.as_ref().is_some_and(|f| *f != sha),
            head_sha: sha,
            result,
            message,
            elapsed_s: self.start.elapsed().as_secs(),
            counts: Counts::of(&checks),
            checks,
        }
    }

    fn selected(&self, snap: &Snapshot) -> Vec<Check> {
        snap.checks
            .iter()
            .filter(|c| self.options.selection.selects(c))
            .cloned()
            .collect()
    }
}

/// Event names are `&'static str`; the state log stores them as strings.
fn event_name(name: &str) -> Option<&'static str> {
    [
        "check_registered",
        "check_completed",
        "head_changed",
        "base_touched",
    ]
    .into_iter()
    .find(|n| *n == name)
}

impl Watch for CiWait {
    type Doc = Doc;

    fn label(&self) -> String {
        format!("ci {}#{}", self.repo, self.number)
    }

    fn poll(
        &mut self,
        client: &mut Client,
        interval: Duration,
        events: &mut Vec<Event>,
    ) -> Result<Poll<Doc>, Halt> {
        let snap = self.snapshot(client, interval)?;
        self.new_events(&snap, events);
        let sha = snap.sha.clone().unwrap_or_default();
        if self.sha.as_deref() != Some(&sha) {
            if self.sha.is_some() {
                self.sha_since = Instant::now();
            }
            self.first_sha.get_or_insert_with(|| sha.clone());
            self.sha = Some(sha.clone());
        }
        let selected = self.selected(&snap);
        let pr = snap.pr.clone();
        let polled_at = snap.polled_at_ms;
        self.last = snap;
        // Measured between real polls, so a snapshot another wait fetched a
        // moment ago does not count as a second report.
        let conflicted = pr.as_ref().filter(|p| p.conflicted());
        if self.conflict.confirmed(conflicted.is_some(), polled_at)
            && let Some(pr) = conflicted
        {
            let message = format!(
                "{}#{} conflicts with {}; pull_request workflows will not run until it is rebased",
                self.repo, self.number, pr.base_ref
            );
            return Ok(Poll::Done(self.doc("conflict", selected, Some(message))));
        }
        if conflicted.is_some() {
            return Ok(Poll::Pending(watch::CONFIRMING.into()));
        }
        if selected.is_empty() {
            if self.sha_since.elapsed() >= self.options.appear_timeout {
                let message = format!(
                    "no checks registered on {} within {}",
                    short(&sha),
                    elapsed(self.options.appear_timeout.as_secs())
                );
                return Ok(Poll::Done(self.doc("no_checks", selected, Some(message))));
            }
            return Ok(Poll::Pending(format!(
                "waiting for checks on {}",
                short(&sha)
            )));
        }
        Ok(match checks::verdict(&selected, self.options.all) {
            Verdict::Success => Poll::Done(self.doc("success", selected, None)),
            Verdict::Failure => {
                let mut selected = selected;
                if let Some(lines) = self.options.log_lines {
                    attach_logs(client, &self.repo, &mut selected, lines);
                }
                if self.options.local_runner {
                    checks::suggest_local(&mut selected, None);
                }
                Poll::Done(self.doc("failure", selected, None))
            }
            Verdict::Pending => Poll::Pending(Counts::of(&selected).summary()),
        })
    }

    fn timed_out(&mut self, _client: &mut Client) -> Doc {
        self.doc("timeout", self.selected(&self.last), None)
    }

    fn text(&self, doc: &Doc, reporter: &Reporter) -> String {
        let (word, good) = match doc.result {
            "success" => ("PASS", true),
            "failure" => ("FAIL", false),
            "conflict" => ("CONFLICT", false),
            "no_checks" => ("NO_CHECKS", false),
            _ => ("PENDING", false),
        };
        let mut out = format!(
            "{} {}#{} @ {}  {}  ({})\n",
            reporter.word(word, good),
            doc.repo,
            doc.pr,
            short(&doc.head_sha),
            doc.counts.summary(),
            elapsed(doc.elapsed_s)
        );
        if let Some(message) = &doc.message {
            let _ = writeln!(out, "  {message}");
        }
        if doc.head_changed {
            out.push_str("  the head moved while waiting; these are the new head's checks\n");
        }
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

pub(crate) fn result_exit(result: &str) -> Exit {
    match result {
        "success" => Exit::Ok,
        "conflict" => Exit::Precondition,
        "timeout" => Exit::Timeout,
        _ => Exit::Failed,
    }
}

/// Fills in the failed step, workflow and log excerpt of each failed
/// Actions job. A log that cannot be fetched is reported on stderr and
/// leaves the check with its name, conclusion and URL.
pub(crate) fn attach_logs(client: &mut Client, repo: &Repo, checks: &mut [Check], lines: usize) {
    let mut cache = BTreeMap::new();
    for check in checks.iter_mut().filter(|c| c.state.failed()) {
        let Some(job_id) = check.job_id else {
            continue;
        };
        if let Err(e) = attach_log(client, &mut cache, repo, check, job_id, lines) {
            eprintln!(
                "jouet: could not fetch the log of {} (job {job_id}): {}",
                check.name,
                e.message()
            );
        }
    }
}

fn attach_log(
    client: &mut Client,
    cache: &mut BTreeMap<String, watch::Cached>,
    repo: &Repo,
    check: &mut Check,
    job_id: u64,
    lines: usize,
) -> Result<(), Failure> {
    let base = format!("/repos/{}/{}/actions/jobs/{job_id}", repo.owner, repo.name);
    let job = watch::get_json(client, cache, &base)?
        .ok_or_else(|| Failure::Other("the job does not exist".into()))?;
    let step = Step::failed(&job);
    check.failed_step = step.as_ref().map(|s| s.name.clone());
    check.workflow = check
        .workflow
        .take()
        .or_else(|| job["workflow_name"].as_str().map(str::to_string));
    check.run_id = check.run_id.or_else(|| job["run_id"].as_u64());
    let dir = jouet_output::paths::log_dir();
    let path = dir.join(format!("{}-{}-job-{job_id}.log", repo.owner, repo.name));
    // A finished job's log never changes, so it is fetched once.
    let log = match std::fs::read_to_string(&path) {
        Ok(log) => log,
        Err(_) => {
            let resp = client.get(&format!("{base}/logs"), None)?;
            if resp.status != 200 {
                return Err(Failure::Other(format!("GitHub returned {}", resp.status)));
            }
            std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&path, &resp.body))
                .map_err(|e| Failure::Other(format!("saving {}: {e}", path.display())))?;
            resp.body
        }
    };
    let excerpt = joblog::excerpt(&log, step.as_ref(), lines);
    check.log_excerpt = excerpt.lines;
    check.log_truncated = excerpt.truncated;
    check.log_path = Some(path.display().to_string());
    Ok(())
}
