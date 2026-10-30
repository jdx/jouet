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
    Ok(jouet_git::status::parse(&out))
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
