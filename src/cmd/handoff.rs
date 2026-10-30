//! `jouet handoff` and `jouet pickup`.

use super::Output;
use crate::{HandoffArgs, PickupArgs};
use jouet_git::Git;
use jouet_git::handoff::{self, Attention, Drift, Handoff, Options, Pr};
use jouet_git::session;
use jouet_github::Client;
use jouet_github::pr::status::{self, Lookup};
use jouet_github::pr::target::{self, Target};
use jouet_output::exit::Exit;
use jouet_output::time;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;

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
        session: session.map(|id| (jouet_output::paths::sessions_dir(), id)),
        processes: true,
    };
    let mut h = match handoff::collect(&git, &opts) {
        Ok(h) => h,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
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
    let drift = match handoff::drift(&git, &h) {
        Ok(drift) => drift,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    let report = PickupReport {
        path,
        accurate: drift.is_empty(),
        drift: &drift,
        handoff: &h,
    };
    out.result(&report, || {
        let mut s = h.markdown(false);
        let age = time::parse_utc(&h.created_at)
            .map(|t| format!(" ({} ago)", time::age(time::now() - t)))
            .unwrap_or_default();
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
