//! The few facts jouet reads from the git checkout it runs in.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// The full SHA of HEAD.
pub(crate) fn head() -> Option<String> {
    git(&["rev-parse", "--verify", "--quiet", "HEAD"])
}

/// The abbreviated SHA of HEAD, as git would print it.
pub(crate) fn short_head() -> Option<String> {
    git(&["rev-parse", "--verify", "--quiet", "--short", "HEAD"])
}

/// Whether `ancestor` is `commit` or one of its ancestors; `None` when git
/// cannot tell, such as when either commit is not in this clone.
pub(crate) fn is_ancestor(ancestor: &str, commit: &str) -> Option<bool> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, commit])
        .output()
        .ok()?
        .status;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// The top of the working tree.
pub(crate) fn toplevel() -> Option<PathBuf> {
    git(&["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Whether `path` (relative to the top of the tree) matches HEAD.
pub(crate) fn unchanged(top: &Path, path: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["status", "--porcelain", "--", path])
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout.is_empty())
}

/// The root of the main checkout when the current directory is in one of its
/// linked worktrees, or the main checkout itself; `None` outside a
/// repository, in a bare one, or in a submodule.
pub(crate) fn main_checkout() -> Option<PathBuf> {
    let common = PathBuf::from(git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ])?);
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

/// Whether git ignores `path` (relative to `top`); `None` when git cannot
/// tell, such as outside a repository.
pub(crate) fn ignored(top: &Path, path: &str) -> Option<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["check-ignore", "--quiet", "--", path])
        .output()
        .ok()?
        .status;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}
