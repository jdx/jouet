//! `jouet ci run`: a workflow's jobs on this machine, through `jactionlint
//! run`, reported the way `wait ci` reports GitHub's.
//!
//! jactionlint does the running (a copy of the checkout, the OS sandbox,
//! no container engine). jouet finds the workflow and job behind the names
//! GitHub shows, keeps the whole transcript in a log, and prints each
//! failed step's output cut to what an agent needs.

pub(crate) mod plan;
pub(crate) mod transcript;

use jouet_output::exit::Exit;
use serde::Serialize;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;
use transcript::Job;

const BIN: &str = "jactionlint";

pub(crate) struct Request {
    pub(crate) workflow: Option<String>,
    pub(crate) jobs: Vec<String>,
    pub(crate) check: Option<String>,
    pub(crate) event: Option<String>,
    /// Arguments for `jactionlint run`, as they are.
    pub(crate) extra: Vec<String>,
    /// Lines of each failed step to show; 0 shows them all.
    pub(crate) log_lines: usize,
    /// Copy the transcript to stderr as it arrives.
    pub(crate) live: bool,
    pub(crate) log_dir: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) exit: Exit,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

impl Refusal {
    fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    /// `success`, `failure`, or `error` when jactionlint refused the
    /// arguments or could not start the run.
    pub(crate) result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workflow: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) selected: Vec<String>,
    /// The `jactionlint` command line that ran.
    pub(crate) command: Vec<String>,
    /// jactionlint's exit code; none when a signal ended it.
    pub(crate) exit_code: Option<i32>,
    pub(crate) jobs: Vec<Job>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) warnings: Vec<String>,
    /// IDs for `jactionlint run --explain`, from every pointer the run
    /// printed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) explain: Vec<String>,
    /// The end of the output when no job ran, such as lint errors that
    /// stopped the run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) output: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) output_truncated: bool,
    pub(crate) log_path: String,
    pub(crate) elapsed_s: u64,
}

impl Report {
    pub(crate) fn exit(&self) -> Exit {
        match self.exit_code {
            Some(0) => Exit::Ok,
            Some(2) => Exit::Usage,
            Some(3) => Exit::Precondition,
            _ => Exit::Failed,
        }
    }
}

pub(crate) fn run(req: &Request, cwd: &Path) -> Result<Report, Refusal> {
    probe(cwd)?;
    let (workflow, mut jobs) = target(req, cwd)?;
    jobs.extend(req.jobs.iter().cloned());
    let mut args = vec!["run".to_string()];
    args.extend(workflow.iter().cloned());
    for job in &jobs {
        args.extend(["-j".to_string(), job.clone()]);
    }
    if let Some(event) = &req.event {
        args.extend(["-e".to_string(), event.clone()]);
    }
    args.extend(req.extra.iter().cloned());
    let log_path = log_path(&req.log_dir);
    let start = Instant::now();
    let (code, text) = stream(&args, cwd, &log_path, req.live)?;
    let mut parsed = transcript::parse(&text);
    let result = match code {
        Some(0) => "success",
        Some(2 | 3) => "error",
        _ => "failure",
    };
    for step in parsed.jobs.iter_mut().flat_map(|j| &mut j.failed_steps) {
        let e = jouet_wait::trim_excerpt(std::mem::take(&mut step.output), req.log_lines);
        step.excerpt = e.lines;
        step.excerpt_truncated = e.truncated;
    }
    let (output, output_truncated) = if parsed.jobs.is_empty() {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let e = jouet_wait::trim_excerpt(lines, req.log_lines);
        (e.lines, e.truncated)
    } else {
        (Vec::new(), false)
    };
    let mut command = vec![BIN.to_string()];
    command.extend(masked(&args));
    Ok(Report {
        result,
        workflow,
        selected: jobs,
        command,
        exit_code: code,
        jobs: parsed.jobs,
        warnings: parsed.warnings,
        explain: parsed.explain,
        output,
        output_truncated,
        log_path: log_path.display().to_string(),
        elapsed_s: start.elapsed().as_secs(),
    })
}

/// Refuses early when jactionlint is missing or has no `run`.
fn probe(cwd: &Path) -> Result<(), Refusal> {
    let out = Command::new(BIN)
        .args(["run", "--version"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output();
    match out {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Refusal::new(
            Exit::Precondition,
            "jactionlint is not installed; it runs the workflow",
        )
        .hint("mise use -g jactionlint")),
        Err(e) => Err(Refusal::new(
            Exit::Precondition,
            format!("running jactionlint: {e}"),
        )),
        Ok(out)
            if out.status.success()
                && String::from_utf8_lossy(&out.stdout).starts_with("jactionlint run") =>
        {
            Ok(())
        }
        Ok(_) => Err(Refusal::new(
            Exit::Precondition,
            "this jactionlint cannot run workflows: it has no `run` command",
        )
        .hint("upgrade it: mise up jactionlint")),
    }
}

/// The workflow to pass and the jobs `--check` adds. A workflow that is a
/// file is passed as it is; a `name:` or a check is looked up in the plan.
fn target(req: &Request, cwd: &Path) -> Result<(Option<String>, Vec<String>), Refusal> {
    let is_file = |w: &str| {
        let dir = cwd.join(".github/workflows");
        cwd.join(w).is_file()
            || ["", ".yml", ".yaml"]
                .iter()
                .any(|ext| dir.join(format!("{w}{ext}")).is_file())
    };
    let named = req.workflow.as_deref().filter(|w| !is_file(w));
    if named.is_none() && req.check.is_none() {
        return Ok((req.workflow.clone(), Vec::new()));
    }
    let mut plans =
        plan::parse(&list(req, cwd, true)?).map_err(|e| Refusal::new(Exit::Failed, e))?;
    let chosen = match named {
        Some(w) => Some(plan::workflow(&plans, w).ok_or_else(|| {
            let known: Vec<String> = plans
                .iter()
                .map(|p| match &p.name {
                    Some(n) => format!("{} ({n})", p.workflow),
                    None => p.workflow.clone(),
                })
                .collect();
            Refusal::new(Exit::NotFound, format!("no workflow is named `{w}`"))
                .hint(format!("the workflows: {}", known.join(", ")))
        })?),
        None => None,
    };
    let workflow = chosen.map(|p| p.workflow.clone()).or(req.workflow.clone());
    let Some(check) = &req.check else {
        return Ok((workflow, Vec::new()));
    };
    let chosen = chosen.map(|p| p.workflow.clone());
    let mut lookup = find_check(&plans, chosen.as_deref(), req.workflow.as_deref(), check);
    if matches!(lookup, plan::Lookup::Missing(_)) && !plan::has_display_names(&plans) {
        plan::add_display_names(&mut plans, &list(req, cwd, false)?);
        lookup = find_check(&plans, chosen.as_deref(), req.workflow.as_deref(), check);
    }
    match lookup {
        plan::Lookup::Found { workflow, job } => Ok((Some(workflow), vec![job])),
        plan::Lookup::Ambiguous(files) => Err(Refusal::new(
            Exit::Usage,
            format!(
                "`{check}` is a job in several workflows: {}",
                files.join(", ")
            ),
        )
        .hint(format!(
            "name the workflow: jouet ci run {} --check '{check}'",
            files[0]
        ))),
        plan::Lookup::Missing(names) => {
            const SHOWN: usize = 20;
            let mut list = names
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if names.len() > SHOWN {
                list.push_str(&format!(", and {} more", names.len() - SHOWN));
            }
            Err(Refusal::new(
                Exit::NotFound,
                format!("no job matches the check `{check}`"),
            )
            .hint(format!("the jobs: {list}; or give the job with -j ID")))
        }
    }
}

/// Looks for the check in the workflow chosen by name, the one given as a
/// file, or else every workflow.
fn find_check(
    plans: &[plan::Plan],
    chosen: Option<&str>,
    given: Option<&str>,
    check: &str,
) -> plan::Lookup {
    let scope: Vec<&plan::Plan> = match (chosen, given) {
        (Some(w), _) => plans.iter().filter(|p| p.workflow == w).collect(),
        (None, Some(w)) => plans
            .iter()
            .filter(|p| plan::workflow(std::slice::from_ref(p), w).is_some())
            .collect(),
        (None, None) => plans.iter().collect(),
    };
    plan::check(&scope, check)
}

/// The plan of every workflow, as JSON or as text.
fn list(req: &Request, cwd: &Path, json: bool) -> Result<String, Refusal> {
    let mut cmd = Command::new(BIN);
    cmd.args(["run", "--list"]);
    if json {
        cmd.arg("--json");
    }
    if let Some(event) = &req.event {
        cmd.args(["-e", event]);
    }
    let out = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Refusal::new(Exit::Precondition, format!("running jactionlint: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Refusal::new(
            Exit::Precondition,
            format!(
                "jactionlint run --list failed: {}",
                err.lines().next().unwrap_or("no message")
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The arguments with the value of each `--secret NAME=VALUE` as `***`.
fn masked(args: &[String]) -> Vec<String> {
    let mask = |a: &str| match a.split_once('=') {
        Some((name, _)) => format!("{name}=***"),
        None => a.to_string(),
    };
    let mut out = Vec::with_capacity(args.len());
    let mut secret_next = false;
    for a in args {
        if secret_next {
            out.push(mask(a));
            secret_next = false;
        } else if let Some(v) = a.strip_prefix("--secret=") {
            out.push(format!("--secret={}", mask(v)));
        } else {
            secret_next = a == "--secret";
            out.push(a.clone());
        }
    }
    out
}

fn log_path(dir: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    dir.join(format!("ci-run-{stamp}-{}.log", std::process::id()))
}

/// Runs jactionlint with its output kept in `log` and returned, stdout
/// and stderr in the order they arrived.
fn stream(
    args: &[String],
    cwd: &Path,
    log: &Path,
    live: bool,
) -> Result<(Option<i32>, String), Refusal> {
    let saving =
        |e: std::io::Error| Refusal::new(Exit::Failed, format!("saving {}: {e}", log.display()));
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(saving)?;
    }
    let mut file = std::fs::File::create(log).map_err(saving)?;
    let mut child = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Refusal::new(Exit::Precondition, format!("running jactionlint: {e}")))?;
    let (tx, rx) = mpsc::channel::<String>();
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|r| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
    })
    .collect();
    drop(tx);
    let mut text = String::new();
    for line in rx {
        let _ = writeln!(file, "{line}");
        if live {
            eprintln!("{line}");
        }
        text.push_str(&line);
        text.push('\n');
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child
        .wait()
        .map_err(|e| Refusal::new(Exit::Failed, format!("waiting for jactionlint: {e}")))?;
    Ok((status.code(), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_values_are_masked_in_the_reported_command() {
        let args: Vec<String> = [
            "run",
            "ci",
            "--secret",
            "TOKEN=abc",
            "--secret=KEY=a=b",
            "--var",
            "A=1",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            masked(&args),
            [
                "run",
                "ci",
                "--secret",
                "TOKEN=***",
                "--secret=KEY=***",
                "--var",
                "A=1"
            ]
        );
    }
}
