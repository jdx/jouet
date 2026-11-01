//! `jouet secrets scan`, and the secret scan `jouet commit` runs before it
//! commits: gitleaks' rules (jouet-secrets), and in a fnox project, fnox's
//! check for its secrets' values, over the one temporary index the commit
//! builds.

use super::Output;
use crate::cli::{SecretsArgs, SecretsCmd, SecretsScanArgs};
use crate::fnox;
use jouet_git::commit::{ScanStatus, Scanner as CommitScanner, SecretFinding, SecretScan};
use jouet_output::exit::Exit;
use jouet_secrets::{Report, Scanner, Source};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) fn run(args: &SecretsArgs, out: &Output) -> Exit {
    match &args.cmd {
        SecretsCmd::Scan(args) => scan(args, out),
    }
}

fn scan(args: &SecretsScanArgs, out: &Output) -> Exit {
    if args.staged && args.diff.is_some() {
        return out.error(Exit::Usage, "give --staged or --diff, not both");
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("the current directory: {e}")),
    };
    let repo = jouet_output::paths::work_tree(&cwd);
    let git = args.staged || args.diff.is_some();
    let root = match (&repo, git) {
        (Some(root), _) => root.clone(),
        (None, false) => cwd.clone(),
        (None, true) => {
            return out.error_with_hint(
                Exit::Precondition,
                "--staged and --diff need a git repository",
                Some("run it inside one, or name the files to scan without them"),
            );
        }
    };
    let source = if args.staged {
        Source::Staged {
            paths: args.paths.clone(),
            index: None,
        }
    } else if let Some(rev) = &args.diff {
        match revs(&cwd, rev) {
            Ok(revs) => Source::Commits {
                revs,
                paths: args.paths.clone(),
            },
            Err((exit, message)) => return out.error(exit, &message),
        }
    } else {
        Source::Files(args.paths.iter().map(PathBuf::from).collect())
    };
    let report = Scanner::for_repo(&root).and_then(|s| s.scan(&root, &cwd, &source));
    let report = match report {
        Ok(report) => report,
        Err(e) => return out.error(e.exit, &e.message),
    };
    out.result(&report, || render(&report));
    if report.findings.is_empty() {
        Exit::Ok
    } else {
        Exit::Failed
    }
}

/// `git log` arguments for --diff: a range as given, else `REF..HEAD`. A
/// name that is not a commit is not found.
fn revs(cwd: &Path, rev: &str) -> Result<Vec<String>, (Exit, String)> {
    if rev.starts_with('-') {
        return Err((Exit::Usage, format!("--diff takes a ref, not `{rev}`")));
    }
    let ends: Vec<&str> = match rev.split_once("..") {
        Some((a, b)) => vec![a, b.trim_start_matches('.')],
        None => vec![rev],
    };
    for end in ends.iter().filter(|e| !e.is_empty()) {
        let ok = std::process::Command::new("git")
            .current_dir(cwd)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{end}^{{commit}}"),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err((Exit::NotFound, format!("no commit `{end}`")));
        }
    }
    Ok(vec![if rev.contains("..") {
        rev.to_string()
    } else {
        format!("{rev}..HEAD")
    }])
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn render(report: &Report) -> String {
    let mut s = String::new();
    let files = plural(report.files_scanned, "file", "files");
    let what = match (report.source, report.commits_scanned) {
        ("staged", _) => plural(report.files_scanned, "staged file", "staged files"),
        (_, Some(commits)) => format!("{files} in {}", plural(commits, "commit", "commits")),
        _ => files,
    };
    let by = format!("{}, {} rules", report.config, report.rules);
    if report.findings.is_empty() {
        let _ = writeln!(s, "no secrets found in {what} ({by})");
    } else {
        let _ = writeln!(
            s,
            "found {} in {what} ({by})",
            plural(report.findings.len(), "secret", "secrets")
        );
        let place = |f: &jouet_secrets::Finding| {
            let at = if f.line == 0 {
                f.path.clone()
            } else {
                format!("{}:{}", f.path, f.line)
            };
            match &f.commit {
                Some(c) => format!("{} {at}", c.get(..12).unwrap_or(c)),
                None => at,
            }
        };
        let width = report
            .findings
            .iter()
            .map(|f| place(f).len())
            .max()
            .unwrap_or(0);
        let rule_width = report
            .findings
            .iter()
            .map(|f| f.rule.len())
            .max()
            .unwrap_or(0);
        for f in &report.findings {
            let _ = writeln!(
                s,
                "  {:<width$}  {:<rule_width$}  {}",
                place(f),
                f.rule,
                f.redacted
            );
        }
        s.push_str("  a false positive: add `gitleaks:allow` to its line, or its fingerprint (PATH:RULE:LINE, --json lists them) to .gitleaksignore\n");
    }
    if report.ignored > 0 {
        let _ = writeln!(
            s,
            "  {} passed over (.gitleaksignore or --allow-secret)",
            plural(report.ignored, "finding", "findings")
        );
    }
    if report.binary_files > 0 {
        let _ = writeln!(
            s,
            "  {} checked by name only",
            plural(report.binary_files, "binary file", "binary files")
        );
    }
    for skipped in &report.skipped {
        let _ = writeln!(s, "  not scanned: {} ({})", skipped.path, skipped.reason);
    }
    s
}

/// Where fnox comes into a commit's scan.
pub(crate) enum Fnox {
    /// Not a fnox project.
    No,
    /// A fnox project without fnox installed.
    Missing,
    Scan(fnox::Scan),
}

/// The scan `jouet commit` runs: gitleaks' rules over the lines the commit
/// adds, then fnox's, over the same index.
pub(crate) struct CommitScan {
    /// `--allow-secret`, as fingerprints (`PATH:RULE:LINE`).
    pub allow: Vec<String>,
    pub fnox: Fnox,
}

/// `RULE:PATH:LINE` as the fingerprint `.gitleaksignore` holds,
/// `PATH:RULE:LINE`.
pub(crate) fn allowance(given: &str) -> Result<String, String> {
    let bad = || {
        format!(
            "--allow-secret takes RULE:PATH:LINE, as the refusal lists the finding, not `{given}`"
        )
    };
    let (rule, rest) = given.split_once(':').ok_or_else(bad)?;
    let (path, line) = rest.rsplit_once(':').ok_or_else(bad)?;
    if rule.is_empty() || path.is_empty() || line.parse::<u64>().is_err() {
        return Err(bad());
    }
    Ok(format!("{path}:{rule}:{line}"))
}

impl CommitScanner for CommitScan {
    fn scan(&self, top: &Path, index: &Path) -> Result<SecretScan, jouet_git::Error> {
        let error = |e: jouet_secrets::Error| jouet_git::Error {
            exit: e.exit,
            message: format!(
                "the secret scan could not run, so nothing was committed: {}",
                e.message
            ),
            hint: Some(DISABLE.into()),
        };
        let mut scanner = Scanner::for_repo(top).map_err(error)?;
        for a in &self.allow {
            scanner.allow(a.clone());
        }
        let source = Source::Staged {
            paths: Vec::new(),
            index: Some(index.to_path_buf()),
        };
        let report = scanner.scan(top, top, &source).map_err(error)?;
        let mut findings: Vec<SecretFinding> = report
            .findings
            .iter()
            .map(|f| SecretFinding {
                rule: Some(f.rule.clone()),
                path: f.path.clone(),
                line: f.line,
                redacted: Some(f.redacted.clone()),
                ..SecretFinding::default()
            })
            .collect();
        let mut notes = Vec::new();
        if report.ignored > 0 {
            notes.push(format!(
                "{} passed over (.gitleaksignore or --allow-secret)",
                plural(report.ignored, "finding", "findings")
            ));
        }
        let mut scan = SecretScan {
            tool: "jouet".into(),
            status: ScanStatus::Clean,
            files_scanned: Some(report.files_scanned as u64),
            findings: Vec::new(),
            reason: None,
            notes,
        };
        match &self.fnox {
            Fnox::No => {}
            Fnox::Missing => scan.reason = Some(fnox::NOT_INSTALLED.into()),
            Fnox::Scan(fnox_scan) => {
                let theirs = fnox_scan.scan(top, index)?;
                scan.tool = "jouet+fnox".into();
                findings.extend(theirs.findings);
                scan.notes
                    .extend(theirs.notes.into_iter().map(|n| format!("fnox: {n}")));
                scan.reason = theirs.reason;
                // A fnox scan that could not run is no clean scan.
                if theirs.status == ScanStatus::Skipped {
                    scan.status = ScanStatus::Skipped;
                }
            }
        }
        if !findings.is_empty() {
            scan.status = ScanStatus::Found;
        }
        scan.findings = findings;
        Ok(scan)
    }
}

const DISABLE: &str = "fix what it reports, or set [commit] secret_scan = false (JOUET_COMMIT_SECRET_SCAN=0) to commit without the scan";
