//! `jouet handoff` and `jouet pickup`.

use super::Output;
use crate::{HandoffArgs, PickupArgs};
use jouet_git::Git;
use jouet_git::handoff::{
    self, Attention, Daemon, DaemonNow, Drift, Handoff, Job, JobNow, Options, Pr,
};
use jouet_git::session;
use jouet_github::Client;
use jouet_github::pr::status::{self, Lookup};
use jouet_github::pr::target::{self, Target};
use jouet_output::exit::Exit;
use jouet_output::time;
use jouet_wait::job::Store;
use jouet_wait::pitchfork;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct HandoffReport<'a> {
    /// Absent when the handoff went to stdout.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    handoff: &'a Handoff,
}

pub(crate) fn run_handoff(args: &HandoffArgs, out: &Output) -> Exit {
    let git = Git::here();
    let session = args.session.clone().or_else(session::id_from_env);
    let opts = Options {
        session: session
            .zip(crate::verify::sessions_dir())
            .map(|(id, dir)| (dir, id)),
        processes: true,
    };
    let mut h = match handoff::collect(&git, &opts) {
        Ok(h) => h,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    if let Some(store) = Store::here(Path::new(&h.worktree)) {
        let jobs = jobs_here(&store);
        // A job's processes are listed once, as the job.
        h.processes
            .retain(|p| !jobs.iter().any(|j| j.pid == p.pid && j.state == "running"));
        h.jobs = jobs;
    }
    match daemons(Path::new(&h.worktree)) {
        Ok(found) => {
            // A daemon's process is listed once, as the daemon.
            h.processes
                .retain(|p| !found.iter().any(|d| d.pid == Some(p.pid)));
            h.daemons = found;
        }
        Err(Some(e)) => h.notes.push(format!("pitchfork daemons: {e}")),
        Err(None) => {}
    }
    if args.no_pr {
        h.notes.push("pr: not looked up (--no-pr)".into());
    } else {
        match h.branch.clone() {
            Some(branch) => match lookup_pr(&branch) {
                Ok(pr) => h.pr = pr,
                Err(e) => h.notes.push(format!("pr: {e}")),
            },
            None => h
                .notes
                .push("pr: not looked up, since HEAD is detached".into()),
        }
    }

    if args.output.as_deref() == Some("-") {
        let report = HandoffReport {
            path: None,
            handoff: &h,
        };
        out.result(&report, || h.markdown(true));
        return Exit::Ok;
    }
    let path = match &args.output {
        Some(path) => PathBuf::from(path),
        None => match handoff::default_path(&git) {
            Ok(path) => path,
            Err(e) => return out.error(e.exit, &e.message),
        },
    };
    if let Err(e) = handoff::write(&path, &h) {
        return out.error(Exit::Failed, &format!("writing {}: {e}", path.display()));
    }
    let path = std::path::absolute(&path).unwrap_or(path);
    let report = HandoffReport {
        path: Some(path.display().to_string()),
        handoff: &h,
    };
    out.result(&report, || {
        format!(
            "{}\nwrote {}; the next session reads it with `jouet pickup`\n",
            h.markdown(false),
            path.display()
        )
    });
    Exit::Ok
}

/// The worktree's jobs, as a handoff records them.
fn jobs_here(store: &Store) -> Vec<Job> {
    store
        .list()
        .into_iter()
        .map(|j| {
            let row = super::job::row(store, j);
            Job {
                id: row.id,
                state: row.state.to_string(),
                exit_code: row.exit_code,
                pid: row.pid,
                command: super::job::shell_words(&row.command),
                log: row.log,
            }
        })
        .collect()
}

/// The pitchfork daemons in the worktree's namespace that are running, on
/// their way up or down, or failed; `Err(None)` without pitchfork. Daemons
/// in `global`, where pitchfork puts those started outside a project, are
/// left out: nothing says which checkout they belong to.
fn daemons(top: &Path) -> Result<Vec<Daemon>, Option<String>> {
    let listed = match pitchfork::list_project(Some(top)) {
        Ok(listed) => listed,
        Err(pitchfork::Error::Missing) => return Err(None),
        Err(e) => return Err(Some(e.message())),
    };
    Ok(listed
        .into_iter()
        .filter(|d| d.namespace != "global")
        .filter(|d| {
            matches!(
                d.phase(),
                pitchfork::Phase::Up | pitchfork::Phase::Moving | pitchfork::Phase::Failed
            )
        })
        .map(recorded)
        .collect())
}

/// What a handoff keeps of a daemon.
fn recorded(d: pitchfork::Daemon) -> Daemon {
    Daemon {
        id: d.id,
        status: d.status,
        pid: d.pid,
        port: d.port,
        error: d.error,
    }
}

/// The open PR for the branch, else its newest, with its attention codes.
fn lookup_pr(branch: &str) -> Result<Option<Pr>, String> {
    let repo = target::repo_from_git().ok_or("no GitHub remote in this checkout")?;
    let mut client = Client::from_env();
    let lookup = Lookup::Pr(Target::Branch(repo, branch.to_string()));
    let fetched = status::fetch(&mut client, vec![lookup])?;
    let now = time::now();
    Ok(fetched.prs.into_iter().next().map(|p| {
        let s = p.finish(now);
        Pr {
            repo: s.repo,
            number: s.number,
            title: s.title,
            url: s.url,
            state: s.state.as_str().to_string(),
            attention: s
                .attention
                .into_iter()
                .map(|a| Attention {
                    code: a.code.to_string(),
                    message: a.message,
                })
                .collect(),
        }
    }))
}

#[derive(Serialize)]
struct PickupReport<'a> {
    path: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    accurate: bool,
    #[serde(skip_serializing_if = "<[Drift]>::is_empty")]
    drift: &'a [Drift],
    /// The jobs the handoff recorded, as they are now.
    #[serde(skip_serializing_if = "<[JobNow]>::is_empty")]
    jobs: &'a [JobNow],
    /// The pitchfork daemons the handoff recorded, as they are now.
    #[serde(skip_serializing_if = "<[DaemonNow]>::is_empty")]
    daemons: &'a [DaemonNow],
    handoff: &'a Handoff,
}

pub(crate) fn run_pickup(args: &PickupArgs, out: &Output) -> Exit {
    let git = Git::here();
    let (path, text) = match read(&git, args.file.as_deref()) {
        Ok(read) => read,
        Err((exit, message)) => return out.error(exit, &message),
    };
    let h = match handoff::parse(&text) {
        Ok(h) => h,
        Err(e) => return out.error(Exit::Precondition, &format!("{path}: {e}")),
    };
    let mut drift = match handoff::drift(&git, &h) {
        Ok(drift) => drift,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    let mut jobs = Vec::new();
    if !h.jobs.is_empty() {
        let now = Store::here(Path::new(&h.worktree))
            .map(|s| jobs_here(&s))
            .unwrap_or_default();
        let (states, ended) = handoff::jobs_now(&h.jobs, &now);
        jobs = states;
        drift.extend(ended);
    }
    let mut daemons = Vec::new();
    if !h.daemons.is_empty() {
        match pitchfork::list_project(Some(Path::new(&h.worktree))) {
            Ok(listed) => {
                let now: Vec<Daemon> = listed.into_iter().map(recorded).collect();
                let (states, changed) = handoff::daemons_now(&h.daemons, &now);
                daemons = states;
                drift.extend(changed);
            }
            Err(e) => drift.push(Drift {
                code: "daemons_unknown",
                message: format!(
                    "the handoff recorded pitchfork daemons, but {}",
                    e.message()
                ),
            }),
        }
    }
    let report = PickupReport {
        path,
        accurate: drift.is_empty(),
        drift: &drift,
        jobs: &jobs,
        daemons: &daemons,
        handoff: &h,
    };
    out.result(&report, || {
        let mut s = h.markdown(false);
        let age = time::parse_utc(&h.created_at)
            .map(|t| format!(" ({} ago)", time::age(time::now() - t)))
            .unwrap_or_default();
        if !jobs.is_empty() {
            let _ = writeln!(s, "\n## Jobs now\n");
            for j in &jobs {
                let _ = writeln!(s, "- `{}`: {}, was {}", j.id, j.now, j.then);
            }
        }
        if !daemons.is_empty() {
            let _ = writeln!(s, "\n## pitchfork daemons now\n");
            for d in &daemons {
                let now = match &d.error {
                    Some(error) => format!("{} ({error})", d.now),
                    None => d.now.clone(),
                };
                let _ = writeln!(s, "- `{}`: {now}, was {}", d.id, d.then);
            }
        }
        if drift.is_empty() {
            let _ = writeln!(
                s,
                "\nStill accurate: nothing has drifted since {}{age}. The PR is as of then; `jouet pr status` has its current state.",
                h.created_at
            );
        } else {
            let _ = writeln!(s, "\n## Drift since {}{age}\n", h.created_at);
            for d in &drift {
                let _ = writeln!(s, "- `{}`: {}", d.code, d.message);
            }
        }
        s
    });
    if drift.is_empty() {
        Exit::Ok
    } else {
        Exit::Failed
    }
}

/// The handoff's path (as shown) and text.
fn read(git: &Git, file: Option<&str>) -> Result<(String, String), (Exit, String)> {
    if file == Some("-") {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| (Exit::Failed, format!("reading stdin: {e}")))?;
        return Ok(("-".into(), text));
    }
    let path = match file {
        Some(file) => PathBuf::from(file),
        None => handoff::default_path(git).map_err(|e| (e.exit, e.message))?,
    };
    let shown = path.display().to_string();
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((shown, text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err((
            Exit::NotFound,
            format!("no handoff at {shown}; `jouet handoff` writes one"),
        )),
        Err(e) => Err((Exit::Failed, format!("{shown}: {e}"))),
    }
}
