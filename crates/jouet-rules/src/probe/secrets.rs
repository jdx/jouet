//! `jouet.secrets.scan()`: what gitleaks' rules find in the lines a commit
//! or a push would add, through jouet-secrets. The diff comes from one git
//! run, cut off past [`LIMIT`], and the scan runs within the budget.

use jouet_secrets::{Report, Scanner, Source, diff_args};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A diff longer than this is not scanned: the probe would not finish
/// within its budget, and a commit that large is not a normal one.
pub(crate) const LIMIT: usize = 1 << 20;

/// The findings in what `scope` adds, as JSON: `staged` (the index against
/// HEAD), `tracked` (every tracked change, what `git commit -a` takes) or
/// `outgoing` (each commit not on any remote). `paths`, files a `git add`
/// in the same call is about to stage, are scanned as they are in the work
/// tree, whole if git does not track them yet.
pub(crate) fn scan(
    dir: Option<&Path>,
    scope: &str,
    paths: &[String],
    timeout: Duration,
) -> Result<Option<Value>, String> {
    let started = Instant::now();
    let here = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let root = jouet_output::paths::work_tree(&here).ok_or("not in a git work tree")?;
    let scanner = Scanner::for_repo(&root).map_err(|e| e.message)?;
    let (source, mut args) = match scope {
        "staged" => ("staged", diff_args(&["diff", "--cached"])),
        "tracked" => ("staged", diff_args(&["diff", "HEAD"])),
        _ => {
            let mut args = diff_args(&["log", "-p"]);
            args.extend(["--format=%x00%H", "HEAD", "--not", "--remotes"].map(String::from));
            ("commits", args)
        }
    };
    args.push("--".into());
    let left = |started: Instant| timeout.saturating_sub(started.elapsed());
    let mut report = patch(&scanner, dir, source, &args, left(started))?;
    if !paths.is_empty() && scope != "outgoing" {
        let mut tracked = diff_args(&["diff", "HEAD"]);
        tracked.push("--".into());
        tracked.extend(paths.iter().cloned());
        merge(
            &mut report,
            patch(&scanner, dir, source, &tracked, left(started))?,
        );
        let mut others: Vec<String> = ["ls-files", "--others", "--exclude-standard", "-z", "--"]
            .map(String::from)
            .to_vec();
        others.extend(paths.iter().cloned());
        let words: Vec<&str> = others.iter().map(String::as_str).collect();
        let listed = super::git::git(dir, &words, left(started))?;
        let new: Vec<PathBuf> = listed
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|p| here.join(p))
            .collect();
        let size: u64 = new
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        if size > LIMIT as u64 {
            return Err(too_large());
        }
        if !new.is_empty() {
            let files = scanner
                .scan(&root, &here, &Source::Files(new))
                .map_err(|e| e.message)?;
            merge(&mut report, files);
        }
    }
    if started.elapsed() > timeout {
        return Err("the secret scan did not finish within the budget".into());
    }
    let findings: Vec<Value> = report
        .findings
        .iter()
        .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
        .collect();
    Ok(Some(Value::Array(findings)))
}

fn patch(
    scanner: &Scanner,
    dir: Option<&Path>,
    source: &'static str,
    args: &[String],
    timeout: Duration,
) -> Result<Report, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let text = super::git::git_capped(dir, &words, timeout, LIMIT)?.ok_or_else(too_large)?;
    scanner.patch(source, &text).map_err(|e| e.message)
}

fn merge(into: &mut Report, from: Report) {
    for f in from.findings {
        if !into.findings.contains(&f) {
            into.findings.push(f);
        }
    }
}

fn too_large() -> String {
    format!(
        "the diff is larger than {} MiB, so it was not scanned; run `jouet secrets scan --staged` (or `--diff REF`)",
        LIMIT >> 20
    )
}
