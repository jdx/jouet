//! The git recipes behind `jouet commit`, `jouet review-package`,
//! `jouet handoff` and `jouet pickup`.
//!
//! Everything shells out to the `git` on PATH, so hooks, config, attributes
//! and credentials behave exactly as they do for the agent's own git calls.
//! Pathspecs are always literal (`--literal-pathspecs`): a name such as
//! `*.rs` or `:/` means that file, never a pattern or the whole tree.

pub mod agent;
pub mod claim;
pub mod commit;
pub mod conventional;
pub mod handoff;
mod procs;
pub mod review;
pub mod session;
pub mod stack;
pub mod status;

use jouet_output::exit::Exit;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Why a recipe stopped: the exit code to leave with, what went wrong, and
/// what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

impl Error {
    pub(crate) fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// git, run in one directory.
#[derive(Debug, Clone)]
pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// git in the current directory.
    pub fn here() -> Self {
        Self::new(".")
    }

    /// A git command in this directory, with literal pathspecs and no
    /// prompts.
    pub(crate) fn command<S: AsRef<OsStr>>(&self, args: &[S]) -> Command {
        let mut c = self.bare_command();
        c.arg("--literal-pathspecs").args(args);
        c
    }

    /// A git command in this directory with no prompts, for the few
    /// commands that refuse `--literal-pathspecs`.
    fn bare_command(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(&self.dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null());
        c
    }

    /// Whether git ignores `path` (`git check-ignore`, which reads paths but
    /// refuses `--literal-pathspecs`).
    pub(crate) fn ignores(&self, path: &str) -> bool {
        self.bare_command()
            .args(["check-ignore", "-q", "--", path])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn output<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Output, Error> {
        self.command(args)
            .output()
            .map_err(|e| Error::new(Exit::Failed, format!("running git: {e}")))
    }

    /// The stdout of a git command that must succeed.
    pub(crate) fn text<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<String, Error> {
        let out = self.output(args)?;
        if !out.status.success() {
            let words: Vec<String> = args
                .iter()
                .map(|a| a.as_ref().to_string_lossy().into_owned())
                .collect();
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(Error::new(
                Exit::Failed,
                format!("git {} failed: {}", words.join(" "), stderr.trim()),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// The trimmed stdout of a git command, or `None` when it fails or
    /// prints nothing.
    pub(crate) fn line<S: AsRef<OsStr>>(&self, args: &[S]) -> Option<String> {
        let out = self.output(args).ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }

    /// Whether a git command exits 0.
    pub(crate) fn succeeds<S: AsRef<OsStr>>(&self, args: &[S]) -> bool {
        self.output(args).is_ok_and(|o| o.status.success())
    }

    /// `path` as git in this directory reads it, made absolute.
    pub(crate) fn resolve(&self, path: &str) -> PathBuf {
        let joined = self.dir.join(path);
        std::path::absolute(&joined).unwrap_or(joined)
    }

    /// The top of the working tree, or a precondition error outside one.
    pub fn toplevel(&self) -> Result<PathBuf, Error> {
        self.line(&["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
            .ok_or_else(|| Error::new(Exit::Precondition, "not inside a git working tree"))
    }

    /// This worktree's own git directory (`.git`, or `.git/worktrees/NAME`
    /// in a linked worktree), where jouet keeps the files it writes for a
    /// checkout.
    pub fn git_dir(&self) -> Result<PathBuf, Error> {
        self.line(&["rev-parse", "--absolute-git-dir"])
            .map(PathBuf::from)
            .ok_or_else(|| Error::new(Exit::Precondition, "not inside a git repository"))
    }

    /// The full SHA of a commit, or `None` when it names none.
    pub(crate) fn commit_sha(&self, rev: &str) -> Option<String> {
        self.line(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])
    }

    /// The full SHA of HEAD; `None` on an unborn branch.
    pub(crate) fn head(&self) -> Option<String> {
        self.commit_sha("HEAD")
    }

    /// The branch checked out, if any.
    pub(crate) fn branch(&self) -> Option<String> {
        self.line(&["symbolic-ref", "--quiet", "--short", "HEAD"])
    }

    /// Whether `ancestor` is `commit` or one of its ancestors.
    pub(crate) fn is_ancestor(&self, ancestor: &str, commit: &str) -> bool {
        self.succeeds(&["merge-base", "--is-ancestor", ancestor, commit])
    }

    /// `git rev-list --count RANGE`.
    pub(crate) fn count(&self, range: &str) -> Option<u32> {
        self.line(&["rev-list", "--count", range])?.parse().ok()
    }
}

/// Splits git's `-z` output into its fields.
pub(crate) fn nul_fields(text: &str) -> impl Iterator<Item = &str> {
    text.split('\0').filter(|f| !f.is_empty())
}

/// The last `keep` lines of a log, and how many came before them.
pub(crate) fn tail(text: &str, keep: usize) -> (Vec<String>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(keep);
    (
        lines[skip..].iter().map(|l| (*l).to_string()).collect(),
        skip,
    )
}

/// A new, unique log file in `dir` for a recipe's subprocess output.
pub(crate) fn new_log(dir: &Path, name: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("{name}-{stamp}-{}.log", std::process::id()));
    let file = std::fs::File::create(&path)?;
    Ok((path, file))
}

/// Writes `text` to `path` through a temporary file in the same directory,
/// so a reader never sees half of it.
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(text.as_bytes())?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing {
    //! Throwaway repositories for the tests. Nothing here touches a real
    //! repository: each test gets its own directory, its own identity, and
    //! a hooks directory it controls, so a global `core.hooksPath` cannot
    //! run the developer's hooks.

    use super::Git;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub(crate) struct Repo {
        pub dir: tempfile::TempDir,
    }

    impl Repo {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let repo = Self { dir };
            repo.git(&["init", "-q", "-b", "main"]);
            for (key, value) in [
                ("user.name", "Test"),
                ("user.email", "test@example.com"),
                ("commit.gpgsign", "false"),
                ("core.hooksPath", ".git/hooks"),
                ("core.autocrlf", "false"),
            ] {
                repo.git(&["config", key, value]);
            }
            repo
        }

        pub(crate) fn path(&self) -> &Path {
            self.dir.path()
        }

        pub(crate) fn git_handle(&self) -> Git {
            Git::new(self.path())
        }

        pub(crate) fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        pub(crate) fn write(&self, path: &str, text: &str) -> PathBuf {
            let full = self.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, text).unwrap();
            full
        }

        /// Writes and commits files in one commit.
        pub(crate) fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
            for (path, text) in files {
                self.write(path, text);
                self.git(&["add", "--", path]);
            }
            self.git(&["commit", "-q", "-m", message]);
            self.git(&["rev-parse", "HEAD"])
        }

        /// Installs a hook script (Unix only: hooks are shell scripts).
        #[cfg(unix)]
        pub(crate) fn hook(&self, name: &str, script: &str) {
            use std::os::unix::fs::PermissionsExt;
            let path = self.write(&format!(".git/hooks/{name}"), script);
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}
