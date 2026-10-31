//! Finding secrets (API keys, tokens, private keys) in files, in what is
//! about to be committed, and in commits about to be pushed, with
//! gitleaks' detection rules: `jouet secrets scan`, the scan `jouet commit`
//! runs first, and the `committed-secrets` hook rule.
//!
//! The rules are gitleaks' own `config/gitleaks.toml`, vendored unchanged
//! (`gitleaks/`, MIT), and applied as gitleaks applies them ([`Rules`]): a
//! rule's keywords, regex, secret group, entropy and path, its allowlists
//! and the config's, and `gitleaks:allow` on a line. A repository's
//! `.gitleaks.toml` adds to them or replaces them, and its
//! `.gitleaksignore` lists findings to pass over by fingerprint.
//!
//! A finding names its rule and where it is, with the match redacted: the
//! secret itself is never kept past the check that found it.

mod diff;
mod rules;
#[cfg(test)]
mod tests;

pub use rules::{Rules, gitleaks_version};

use jouet_output::exit::Exit;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Files larger than this are not read, and are reported as skipped.
const MAX_FILE: u64 = 10 << 20;

/// How much of a file is looked at to tell whether it is binary, as git
/// does.
const SNIFF: usize = 8000;

#[derive(Debug)]
pub struct Error {
    pub exit: Exit,
    pub message: String,
}

impl Error {
    fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
        }
    }
}

/// One secret a rule found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// The gitleaks rule, such as `github-pat`.
    pub rule: String,
    pub description: String,
    /// Relative to the repository's root.
    pub path: String,
    /// 1-based; absent for a rule that matches the file's name alone.
    #[serde(skip_serializing_if = "is_zero")]
    pub line: u64,
    /// The commit that added it, when commits were scanned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The match with the secret cut to its first four characters at most:
    /// `api_key = "abcd****`.
    pub redacted: String,
    /// What a line of `.gitleaksignore` holds to pass over it:
    /// `PATH:RULE:LINE`, after `COMMIT:` for a commit's finding.
    pub fingerprint: String,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A file that was not scanned, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// What a scan looked at and found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `files`, `staged` or `commits`.
    pub source: &'static str,
    pub findings: Vec<Finding>,
    /// Files read, or files a diff adds lines to.
    pub files_scanned: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_scanned: Option<usize>,
    /// Binary files, which only path rules look at.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub binary_files: usize,
    /// Findings `.gitleaksignore` or an allowance passed over.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub ignored: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
    /// How many rules ran, and where they came from.
    pub rules: usize,
    pub config: String,
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

/// What to scan.
#[derive(Debug, Clone)]
pub enum Source {
    /// Files and directories as they are on disk; directories are walked
    /// as git sees them, ignored files left out.
    Files(Vec<PathBuf>),
    /// The lines the index adds to HEAD, in `paths` (all of it when
    /// empty). `index` is another index file to read, as `jouet commit`'s.
    Staged {
        paths: Vec<String>,
        index: Option<PathBuf>,
    },
    /// The lines each commit in `revs` adds (`git log` arguments, such as
    /// `main..HEAD`), commit by commit, so a secret added and removed again
    /// is still found.
    Commits {
        revs: Vec<String>,
        paths: Vec<String>,
    },
}

/// The rules for one repository, its `.gitleaksignore`, and allowances.
pub struct Scanner {
    rules: Rules,
    config: String,
    /// Fingerprints to pass over.
    ignore: BTreeSet<String>,
}

impl Scanner {
    /// The rules for the repository at `root`: gitleaks' defaults, with its
    /// `.gitleaks.toml` and `.gitleaksignore` when it has them.
    pub fn for_repo(root: &Path) -> Result<Self, Error> {
        let config_path = root.join(".gitleaks.toml");
        let (rules, config) = match std::fs::read_to_string(&config_path) {
            Ok(text) => {
                let rules = Rules::with_repo_config(&text)
                    .map_err(|e| Error::new(Exit::Precondition, format!(".gitleaks.toml: {e}")))?;
                (
                    rules,
                    format!("gitleaks {} with .gitleaks.toml", gitleaks_version()),
                )
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                Rules::default_rules(),
                format!("gitleaks {}", gitleaks_version()),
            ),
            Err(e) => {
                return Err(Error::new(
                    Exit::Precondition,
                    format!("reading .gitleaks.toml: {e}"),
                ));
            }
        };
        let mut scanner = Self {
            rules,
            config,
            ignore: BTreeSet::new(),
        };
        if let Ok(text) = std::fs::read_to_string(root.join(".gitleaksignore")) {
            for line in text.lines().map(str::trim) {
                if !line.is_empty() && !line.starts_with('#') {
                    scanner.ignore.insert(line.replace('\\', "/"));
                }
            }
        }
        Ok(scanner)
    }

    /// Passes over the finding with this fingerprint (`PATH:RULE:LINE`,
    /// or with `COMMIT:` first).
    pub fn allow(&mut self, fingerprint: String) {
        self.ignore.insert(fingerprint);
    }

    fn ignored(&self, f: &Finding) -> bool {
        let global = format!("{}:{}:{}", f.path, f.rule, f.line);
        self.ignore.contains(&global) || self.ignore.contains(&f.fingerprint)
    }

    fn empty(&self, source: &'static str) -> Report {
        Report {
            source,
            rules: self.rules.len(),
            config: self.config.clone(),
            ..Report::default()
        }
    }

    /// Scans `text`, a file at `path` that starts at `first_line`.
    fn text(
        &self,
        report: &mut Report,
        path: &str,
        commit: Option<&str>,
        first_line: u64,
        text: &[u8],
    ) -> Result<(), Error> {
        let hits = self.rules.scan(text, path, commit).map_err(rule_error)?;
        for hit in hits {
            self.push(report, path, commit, hit.line + first_line - 1, hit);
        }
        Ok(())
    }

    fn push(
        &self,
        report: &mut Report,
        path: &str,
        commit: Option<&str>,
        line: u64,
        hit: rules::Hit,
    ) {
        let mut fingerprint = format!("{path}:{}:{line}", hit.rule);
        if let Some(commit) = commit {
            fingerprint = format!("{commit}:{fingerprint}");
        }
        let finding = Finding {
            rule: hit.rule,
            description: hit.description,
            path: path.to_string(),
            line,
            commit: commit.map(str::to_string),
            redacted: hit.redacted,
            fingerprint,
        };
        if self.ignored(&finding) {
            report.ignored += 1;
        } else if !report.findings.contains(&finding) {
            report.findings.push(finding);
        }
    }

    /// The rules that match a file's name alone.
    fn path(&self, report: &mut Report, path: &str, commit: Option<&str>) -> Result<(), Error> {
        for hit in self.rules.path_hits(path).map_err(rule_error)? {
            self.push(report, path, commit, 0, hit);
        }
        Ok(())
    }

    /// Scans a patch: `git diff -U0` or `git log -p -U0 --format=%x00%H`,
    /// with `a/` and `b/` prefixes. `source` names what it is.
    pub fn patch(&self, source: &'static str, patch: &[u8]) -> Result<Report, Error> {
        let parsed = diff::parse(patch);
        let mut report = self.empty(source);
        if parsed.commits > 0 || source == "commits" {
            report.commits_scanned = Some(parsed.commits);
        }
        let mut seen = BTreeSet::new();
        for (commit, path) in &parsed.files {
            if self.rules.path_allowed(path).map_err(rule_error)? {
                continue;
            }
            seen.insert(path.clone());
            self.path(&mut report, path, commit.as_deref())?;
        }
        report.files_scanned = seen.len();
        for piece in &parsed.pieces {
            if !seen.contains(&piece.path) {
                continue;
            }
            self.text(
                &mut report,
                &piece.path,
                piece.commit.as_deref(),
                piece.first_line,
                &piece.text,
            )?;
        }
        Ok(report)
    }

    /// Scans `source`, from the directory `cwd` in the repository (or
    /// directory) at `root`.
    pub fn scan(&self, root: &Path, cwd: &Path, source: &Source) -> Result<Report, Error> {
        match source {
            Source::Files(paths) => self.files(root, cwd, paths),
            Source::Staged { paths, index } => {
                let mut args = diff_args(&["diff", "--cached"]);
                args.push("--".into());
                args.extend(paths.iter().cloned());
                let patch = git(cwd, &args, index.as_deref())?;
                self.patch("staged", &patch)
            }
            Source::Commits { revs, paths } => {
                let mut args = diff_args(&["log", "-p"]);
                args.push(format!("--format={}", diff::COMMIT_MARK));
                args.extend(revs.iter().cloned());
                args.push("--".into());
                args.extend(paths.iter().cloned());
                let patch = git(cwd, &args, None)?;
                self.patch("commits", &patch)
            }
        }
    }

    fn files(&self, root: &Path, cwd: &Path, paths: &[PathBuf]) -> Result<Report, Error> {
        let mut report = self.empty("files");
        let given: Vec<PathBuf> = if paths.is_empty() {
            vec![cwd.to_path_buf()]
        } else {
            paths.iter().map(|p| cwd.join(p)).collect()
        };
        for p in &given {
            if !p.exists() {
                return Err(Error::new(
                    Exit::NotFound,
                    format!("{}: no such file or directory", p.display()),
                ));
            }
        }
        let mut walk = ignore::WalkBuilder::new(&given[0]);
        for p in &given[1..] {
            walk.add(p);
        }
        walk.hidden(false)
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".git");
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut seen = BTreeSet::new();
        for entry in walk.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    report.skipped.push(Skipped {
                        path: String::new(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let full = entry.path();
            let shown = relative(&root, full);
            if !seen.insert(shown.clone()) || self.rules.path_allowed(&shown).map_err(rule_error)? {
                continue;
            }
            self.path(&mut report, &shown, None)?;
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if size > MAX_FILE {
                report.skipped.push(Skipped {
                    path: shown,
                    reason: format!("larger than {} MiB", MAX_FILE >> 20),
                });
                continue;
            }
            let text = match std::fs::read(full) {
                Ok(t) => t,
                Err(e) => {
                    report.skipped.push(Skipped {
                        path: shown,
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            report.files_scanned += 1;
            if text[..text.len().min(SNIFF)].contains(&0) {
                report.binary_files += 1;
                continue;
            }
            self.text(&mut report, &shown, None, 1, &text)?;
        }
        report
            .findings
            .sort_by(|a, b| (&a.path, a.line, &a.rule).cmp(&(&b.path, b.line, &b.rule)));
        report.skipped.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(report)
    }
}

fn rule_error(e: String) -> Error {
    Error::new(Exit::Failed, format!("gitleaks rules: {e}"))
}

/// `full` relative to `root`, with `/` between parts; as given when it is
/// outside it.
fn relative(root: &Path, full: &Path) -> String {
    let canonical = full.canonicalize().unwrap_or_else(|_| full.to_path_buf());
    let rel = canonical.strip_prefix(root).unwrap_or(&canonical);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// A diff or log command that prints only added lines, in a form the
/// parser reads whatever the user's git config says.
pub fn diff_args(command: &[&str]) -> Vec<String> {
    [
        "-c",
        "core.quotePath=false",
        "--literal-pathspecs",
        "--no-pager",
    ]
    .iter()
    .chain(command)
    .chain(&[
        "-U0",
        "-M",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ])
    .map(|s| (*s).to_string())
    .collect()
}

/// The patch `git ARGS` prints in `cwd`, reading `index` when given.
fn git(cwd: &Path, args: &[String], index: Option<&Path>) -> Result<Vec<u8>, Error> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let out = command
        .output()
        .map_err(|e| Error::new(Exit::Failed, format!("running git: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "git {} failed: {}",
                args.iter()
                    .find(|a| ["diff", "log"].contains(&a.as_str()))
                    .map_or("", String::as_str),
                stderr.trim()
            ),
        ));
    }
    Ok(out.stdout)
}
