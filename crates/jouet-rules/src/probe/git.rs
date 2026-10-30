//! The git probes: `jouet.git.state()` and `jouet.repo()`, through plain
//! `git`, each run killed when the budget runs out.

use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Runs `git ARGS` in `dir` (this process's directory when None) and
/// returns its stdout, or an error once `timeout` passes. Never takes an
/// optional lock, so a probe cannot get in the way of the agent's own git.
pub(crate) fn git(dir: Option<&Path>, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let mut child = command
        .arg("--no-optional-locks")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("running git: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("git has no stdout")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = tx.send(stdout.read_to_end(&mut out).map(|_| out));
    });
    let Ok(out) = rx.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "`git {}` did not finish within the budget",
            args[0]
        ));
    };
    let status = child.wait().map_err(|e| format!("running git: {e}"))?;
    let out = out.map_err(|e| format!("reading git's output: {e}"))?;
    if !status.success() {
        return Err(format!(
            "`git {}` failed: not in a git repository?",
            args[0]
        ));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The branch, its upstream and how far apart they are, and whether the
/// work tree is clean, from one `git status --porcelain=v2 --branch`.
pub(crate) fn state(dir: Option<&Path>, timeout: Duration) -> Result<Value, String> {
    let out = git(dir, &["status", "--porcelain=v2", "--branch"], timeout)?;
    Ok(parse_status(&out))
}

pub(crate) fn parse_status(out: &str) -> Value {
    let (mut head, mut branch, mut upstream) = (None, None, None);
    let (mut ahead, mut behind): (Option<u64>, Option<u64>) = (None, None);
    let (mut changed, mut untracked, mut conflicted) = (0u64, 0u64, 0u64);
    for line in out.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => {
                    head = Some(value.chars().take(12).collect::<String>());
                }
                "branch.head" if value != "(detached)" => branch = Some(value.to_string()),
                "branch.upstream" => upstream = Some(value.to_string()),
                "branch.ab" => {
                    let mut parts = value.split(' ');
                    ahead = parts
                        .next()
                        .and_then(|a| a.trim_start_matches('+').parse().ok());
                    behind = parts
                        .next()
                        .and_then(|b| b.trim_start_matches('-').parse().ok());
                }
                _ => {}
            }
        } else if line.starts_with("? ") {
            untracked += 1;
        } else if line.starts_with("u ") {
            conflicted += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            changed += 1;
        }
    }
    let mut state = json!({
        "detached": branch.is_none(),
        "dirty": changed + untracked + conflicted > 0,
        "changed": changed,
        "untracked": untracked,
        "conflicted": conflicted,
    });
    for (key, value) in [("head", head), ("branch", branch), ("upstream", upstream)] {
        if let Some(value) = value {
            state[key] = value.into();
        }
    }
    for (key, value) in [("ahead", ahead), ("behind", behind)] {
        if let Some(value) = value {
            state[key] = value.into();
        }
    }
    state
}

/// The work tree's root, whether it is a linked worktree, and the GitHub
/// repository its remotes name.
pub(crate) fn repo(dir: Option<&Path>, timeout: Duration) -> Result<Value, String> {
    let started = std::time::Instant::now();
    let out = git(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
        timeout,
    )?;
    let mut lines = out.lines();
    let (Some(root), Some(git_dir), Some(common)) = (lines.next(), lines.next(), lines.next())
    else {
        return Err("not in a git work tree".into());
    };
    let mut repo = json!({ "root": root, "linked_worktree": git_dir != common });
    // `--get-regexp` exits 1 when no remote matches: a repository without
    // a GitHub remote, which is an answer rather than an error.
    let left = timeout.saturating_sub(started.elapsed());
    let config = git(
        dir,
        &["config", "--get-regexp", r"^remote\..*\.(url|gh-resolved)$"],
        left,
    )
    .unwrap_or_default();
    if let Some(github) = jouet_github::pr::target::pick_remote(&config) {
        repo["owner"] = github.owner.as_str().into();
        repo["name"] = github.name.as_str().into();
        repo["repo"] = github.to_string().into();
    }
    Ok(repo)
}
