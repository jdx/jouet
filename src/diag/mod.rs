//! Diagnostics for the files an agent changed: which errors and warnings
//! they have, from the project's own checks.
//!
//! With an hk config (`hk.pkl`, `.config/hk.pkl` or their `.local`
//! variants), the files go to `hk run check --safe --no-fail-fast --format
//! json --files0-from -`, and hk's normalised diagnostics come back as
//! [`Finding`]s. A step that sets no `diagnostic_format` has no findings, so
//! a failed one is reported with its status and the end of its output.
//! `--safe` makes hk refuse, before anything runs, a config with a command
//! whose effect is unknown or destructive; that is reported as such, never
//! as a pass. Without an hk config, a small fallback runs the language's
//! own checker in its JSON mode ([`fallback`]), or says diagnostics are
//! unavailable.
//!
//! [`delta`] keeps what each file's last check found, so a later check can
//! tell which findings an edit introduced; [`fix`] runs `hk run fix --safe`
//! on edited files.

pub(crate) mod delta;
mod fallback;
pub(crate) mod fix;
mod process;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How much of a raw step's output a report keeps, from the end.
const EXCERPT_LINES: usize = 20;
const EXCERPT_CHARS: usize = 2000;
/// How long a check may take before it is stopped.
pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(600);

/// One finding, as hk normalises it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    /// Relative to the repository root, unless the tool named a file outside
    /// it.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<u64>,
    /// error, warning, note or help.
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub message: String,
    /// The hk step (or fallback checker) that reported it.
    pub source_step: String,
}

/// What one step did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Step {
    pub name: String,
    /// passed, failed, skipped or cancelled, as hk reports it.
    pub status: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub findings: usize,
    /// For a failed step that reported no findings (it sets no
    /// `diagnostic_format`): the end of its output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    /// Nothing found, and every step passed.
    Clean,
    /// A finding, or a failed step.
    Findings,
    /// hk refused to run (`--safe`, or a config it could not load).
    Refused,
    /// No hk config and no fallback checker for these files.
    Unavailable,
}

/// The result of a check.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Report {
    pub status: Status,
    /// `hk`, or the fallback checker that ran.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub files: Vec<String>,
    pub findings: Vec<Finding>,
    /// Findings left out of `findings` by `--max`.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// Why hk refused, or why diagnostics are unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub duration_ms: u64,
    /// hk's whole JSON result, or the fallback's output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

impl Report {
    fn empty(status: Status, files: Vec<String>) -> Self {
        Self {
            status,
            source: String::new(),
            files,
            findings: Vec::new(),
            omitted: 0,
            steps: Vec::new(),
            reason: None,
            duration_ms: 0,
            log_path: None,
        }
    }

    /// The report a check of these files would give, from what the cache
    /// kept of each at its current content.
    pub(crate) fn from_cache(files: Vec<String>, seen: &[&delta::Seen]) -> Self {
        let mut report = Self::empty(Status::Findings, files);
        report.source = "cache".into();
        for s in seen {
            report.findings.extend(s.findings.iter().cloned());
            for name in &s.failed_steps {
                if !report.steps.iter().any(|step| &step.name == name) {
                    report.steps.push(Step {
                        name: name.clone(),
                        status: "failed".into(),
                        findings: 0,
                        excerpt: None,
                    });
                }
            }
        }
        report.settle();
        report
    }

    /// Keeps the first `max` findings, counting the rest in `omitted`.
    pub(crate) fn cut(&mut self, max: usize) {
        if self.findings.len() > max {
            self.omitted += self.findings.len() - max;
            self.findings.truncate(max);
        }
    }

    pub(crate) fn failed_steps(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| s.status == "failed")
    }

    fn settle(&mut self) {
        if matches!(self.status, Status::Refused | Status::Unavailable) {
            return;
        }
        self.status = if self.findings.is_empty() && self.failed_steps().next().is_none() {
            Status::Clean
        } else {
            Status::Findings
        };
    }
}

/// The hk config hk would read in `root`, if any.
pub(crate) fn hk_config(root: &Path) -> Option<PathBuf> {
    if let Some(file) = jouet_output::paths::env_dir("HK_FILE") {
        let file = if file.is_absolute() {
            file
        } else {
            root.join(file)
        };
        return file.is_file().then_some(file);
    }
    [
        "hk.local.pkl",
        ".config/hk.local.pkl",
        "hk.pkl",
        ".config/hk.pkl",
    ]
    .iter()
    .map(|name| root.join(name))
    .find(|path| path.is_file())
}

/// The hk binary: `$JOUET_HK`, else `hk` from `PATH`.
pub(crate) fn hk_program() -> String {
    std::env::var("JOUET_HK")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "hk".to_string())
}

/// Checks `files` (relative to `root`).
pub(crate) fn check(root: &Path, files: Vec<String>, timeout: Duration) -> Report {
    let started = Instant::now();
    if files.is_empty() {
        let mut report = Report::empty(Status::Clean, files);
        report.reason = Some("no files to check".into());
        return report;
    }
    let mut report = if hk_config(root).is_some() {
        check_hk(root, files, timeout)
    } else {
        fallback::check(root, files, timeout)
    };
    report.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    report.settle();
    // The files asked about first, then by position.
    let asked = report.files.clone();
    report.findings.sort_by(|a, b| {
        let rank = |f: &Finding| !asked.contains(&f.file);
        (rank(a), &a.file, a.line, a.col).cmp(&(rank(b), &b.file, b.line, b.col))
    });
    report
}

fn check_hk(root: &Path, files: Vec<String>, timeout: Duration) -> Report {
    let mut report = Report::empty(Status::Findings, files);
    report.source = "hk".into();
    let mut argv: Vec<String> = [
        "run",
        "check",
        "--safe",
        "--no-fail-fast",
        "--format",
        "json",
        "--files0-from",
        "-",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    argv.insert(0, hk_program());
    let ran = match process::run(&argv, root, &nul_list(&report.files), timeout) {
        Ok(ran) => ran,
        Err(e) => {
            report.status = Status::Unavailable;
            report.reason = Some(format!("running {}: {e}", argv[0]));
            return report;
        }
    };
    report.log_path = save_log("diag", &ran.stdout);
    if ran.timed_out {
        report.status = Status::Refused;
        report.reason = Some(format!(
            "hk check ran past {}s and was stopped",
            timeout.as_secs()
        ));
        return report;
    }
    let Ok(result) = serde_json::from_slice::<Value>(&ran.stdout) else {
        report.status = Status::Refused;
        report.reason = Some(format!(
            "hk printed no JSON result (exit {}): {}",
            ran.code.map_or("by a signal".into(), |c| c.to_string()),
            excerpt(
                &String::from_utf8_lossy(&ran.stderr),
                report.log_path.as_deref()
            )
            .unwrap_or_default()
        ));
        return report;
    };
    read_hk_result(root, &result, &mut report);
    report
}

/// Fills `report` from hk's `run_result` document.
pub(crate) fn read_hk_result(root: &Path, result: &Value, report: &mut Report) {
    let steps = result["steps"].as_array().map_or(&[][..], Vec::as_slice);
    if steps.is_empty()
        && result["status"] == "failed"
        && let Some(failure) = result["failure"].as_str()
    {
        // A config hk could not load, or `--safe` refusing an effect: either
        // way nothing ran.
        report.status = Status::Refused;
        report.reason = Some(failure.trim().to_string());
        return;
    }
    for step in steps {
        let name = step["name"].as_str().unwrap_or("?").to_string();
        let status = step["status"].as_str().unwrap_or("unknown").to_string();
        let mut found = 0;
        for d in step["diagnostics"].as_array().into_iter().flatten() {
            found += 1;
            report.findings.push(finding(root, d, &name));
        }
        let excerpt = (status == "failed" && found == 0)
            .then(|| {
                excerpt(
                    step["output"].as_str().unwrap_or(""),
                    report.log_path.as_deref(),
                )
            })
            .flatten();
        report.steps.push(Step {
            name,
            status,
            findings: found,
            excerpt,
        });
    }
}

fn finding(root: &Path, d: &Value, step: &str) -> Finding {
    let start = &d["range"]["start"];
    Finding {
        file: d["path"]
            .as_str()
            .map_or_else(String::new, |p| relative(root, p)),
        line: start["line"].as_u64(),
        col: start["column"].as_u64(),
        severity: d["severity"].as_str().unwrap_or("error").to_string(),
        rule: d["rule"].as_str().map(str::to_string),
        message: d["message"].as_str().unwrap_or("").trim_end().to_string(),
        source_step: d["step"].as_str().unwrap_or(step).to_string(),
    }
}

/// `path` relative to `root` when it lies inside it; as given otherwise.
pub(crate) fn relative(root: &Path, path: &str) -> String {
    let p = lexical(Path::new(path));
    let rel = if p.is_absolute() {
        p.strip_prefix(root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                let canon = root.canonicalize().ok()?;
                p.strip_prefix(&canon).ok().map(Path::to_path_buf)
            })
    } else {
        Some(p.clone())
    };
    match rel {
        Some(rel) => rel.to_string_lossy().replace('\\', "/"),
        None => path.to_string(),
    }
}

/// `path` with `.` and `..` resolved without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The end of a step's output, with a marker saying what was left out and
/// where the whole of it is.
fn excerpt(output: &str, log: Option<&str>) -> Option<String> {
    let output = output.trim_end();
    if output.trim().is_empty() {
        return None;
    }
    let lines: Vec<&str> = output.lines().collect();
    let skip = lines.len().saturating_sub(EXCERPT_LINES);
    let mut kept = lines[skip..].join("\n");
    let mut cut_chars = false;
    if kept.chars().count() > EXCERPT_CHARS {
        let start = kept.chars().count() - EXCERPT_CHARS;
        kept = kept.chars().skip(start).collect();
        cut_chars = true;
    }
    if skip == 0 && !cut_chars {
        return Some(kept);
    }
    let whole = log.map_or(String::new(), |l| format!(" (full output: {l})"));
    let what = if skip > 0 {
        format!("{skip} earlier lines")
    } else {
        "the start of a long line".to_string()
    };
    Some(format!("... {what} left out{whole}\n{kept}"))
}

/// A NUL-separated file list, for `--files0-from -`.
pub(crate) fn nul_list(files: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for file in files {
        bytes.extend_from_slice(file.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// Saves `bytes` to a new file in jouet's log directory and returns its path.
pub(crate) fn save_log(prefix: &str, bytes: &[u8]) -> Option<String> {
    let dir = jouet_output::paths::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("{prefix}-{stamp}-{}.json", std::process::id()));
    std::fs::write(&path, bytes).ok()?;
    Some(path.display().to_string())
}

/// The files `paths` name (relative to `cwd`), relative to `root`. A path
/// outside `root` is refused.
pub(crate) fn resolve(root: &Path, cwd: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for path in paths {
        let rel = relative(root, &cwd.join(path).to_string_lossy());
        if Path::new(&rel).is_absolute() || rel.starts_with("../") {
            return Err(format!(
                "{path} is outside the repository at {}",
                root.display()
            ));
        }
        if !out.contains(&rel) {
            out.push(rel);
        }
    }
    Ok(out)
}

/// The files that differ from the index or HEAD, or are untracked and not
/// ignored, that still exist: what an agent changed and has not committed.
pub(crate) fn changed(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for args in [
        &[
            "ls-files",
            "-z",
            "--modified",
            "--others",
            "--exclude-standard",
        ][..],
        &["diff", "--cached", "--name-only", "-z", "--diff-filter=d"][..],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| format!("running git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args[0],
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        for name in out.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
            let name = String::from_utf8_lossy(name).into_owned();
            if root.join(&name).is_file() && !files.contains(&name) {
                files.push(name);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The text form of a report, with at most `max` findings.
pub(crate) fn render(report: &Report, max: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    match report.status {
        Status::Refused => {
            let _ = writeln!(
                s,
                "hk refused to check: {}",
                report.reason.as_deref().unwrap_or("")
            );
            return s;
        }
        Status::Unavailable => {
            let _ = writeln!(
                s,
                "diagnostics unavailable: {}",
                report.reason.as_deref().unwrap_or("")
            );
            return s;
        }
        Status::Clean | Status::Findings => {}
    }
    for f in report.findings.iter().take(max) {
        let _ = writeln!(s, "{}", finding_line(f));
    }
    let more = report.findings.len().saturating_sub(max) + report.omitted;
    if more > 0 {
        let whole = report
            .log_path
            .as_deref()
            .map_or(" (all of them: --json)".to_string(), |l| {
                format!(" (all of them: --json, or {l})")
            });
        let _ = writeln!(s, "... {more} more findings{whole}");
    }
    for step in report.failed_steps().filter(|s| s.findings == 0) {
        let _ = writeln!(s, "step {} failed:", step.name);
        for line in step.excerpt.as_deref().unwrap_or("(no output)").lines() {
            let _ = writeln!(s, "  {line}");
        }
    }
    // Files no checker covered: never part of a pass.
    if let Some(reason) = report.reason.as_deref() {
        let _ = writeln!(s, "note: {reason}");
    }
    let files = report.files.len();
    let summary = if report.status == Status::Clean {
        format!("clean: {files} {}", plural(files, "file"))
    } else {
        let failed = report.failed_steps().count();
        let found = report.findings.len() + report.omitted;
        format!(
            "{found} {} in {files} {}, {failed} failed {}",
            plural(found, "finding"),
            plural(files, "file"),
            plural(failed, "step"),
        )
    };
    let _ = writeln!(
        s,
        "{summary} ({}, {} ms)",
        if report.source.is_empty() {
            "no checker"
        } else {
            &report.source
        },
        report.duration_ms
    );
    s
}

pub(crate) fn finding_line(f: &Finding) -> String {
    let mut at = f.file.clone();
    if let Some(line) = f.line {
        at.push_str(&format!(":{line}"));
        if let Some(col) = f.col {
            at.push_str(&format!(":{col}"));
        }
    }
    let rule = f
        .rule
        .as_deref()
        .map_or(String::new(), |r| format!(" [{r}]"));
    let message = f.message.lines().next().unwrap_or("");
    format!("{at}: {}: {message}{rule} ({})", f.severity, f.source_step)
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_findings_and_raw_steps_from_hk() {
        let root = Path::new("/repo");
        let result = json!({
            "kind": "run_result", "status": "failed",
            "steps": [
                {"name": "shellcheck", "status": "failed", "diagnostics": [
                    {"step": "shellcheck", "severity": "note", "message": "Double quote\n", "path": "/repo/a.sh",
                     "range": {"start": {"line": 2, "column": 6}}, "rule": "SC2086"}
                ], "output": "In a.sh line 2: ..."},
                {"name": "trailing", "status": "failed", "diagnostics": [], "output": "1:x \n"},
                {"name": "fmt", "status": "skipped", "diagnostics": []}
            ]
        });
        let mut report = Report::empty(Status::Findings, vec!["a.sh".into()]);
        read_hk_result(root, &result, &mut report);
        report.settle();
        assert_eq!(report.status, Status::Findings);
        assert_eq!(
            report.findings,
            [Finding {
                file: "a.sh".into(),
                line: Some(2),
                col: Some(6),
                severity: "note".into(),
                rule: Some("SC2086".into()),
                message: "Double quote".into(),
                source_step: "shellcheck".into(),
            }]
        );
        assert_eq!(report.steps[0].excerpt, None);
        assert_eq!(report.steps[1].excerpt.as_deref(), Some("1:x"));
        assert_eq!(report.failed_steps().count(), 2);
    }

    #[test]
    fn a_refusal_is_not_a_pass() {
        let result = json!({"status": "failed", "failure": "--safe refused to run:\n  clippy.check: effect is unknown", "steps": []});
        let mut report = Report::empty(Status::Findings, vec!["a.rs".into()]);
        read_hk_result(Path::new("/r"), &result, &mut report);
        report.settle();
        assert_eq!(report.status, Status::Refused);
        assert!(report.reason.unwrap().contains("effect is unknown"));
    }

    #[test]
    fn excerpts_say_what_they_left_out() {
        let long: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let cut = excerpt(&long, Some("/tmp/log.json")).unwrap();
        assert!(
            cut.starts_with("... 10 earlier lines left out (full output: /tmp/log.json)\nline 11"),
            "{cut}"
        );
        assert_eq!(excerpt("  \n", None), None);
    }

    #[test]
    fn rendering_marks_cut_findings() {
        let f = |line| Finding {
            file: "a.rs".into(),
            line: Some(line),
            col: None,
            severity: "error".into(),
            rule: None,
            message: "bad".into(),
            source_step: "clippy".into(),
        };
        let mut report = Report::empty(Status::Findings, vec!["a.rs".into()]);
        report.source = "hk".into();
        report.findings = (1..=3).map(f).collect();
        let text = render(&report, 2);
        assert!(text.contains("a.rs:2: error: bad (clippy)"), "{text}");
        assert!(!text.contains("a.rs:3:"), "{text}");
        assert!(
            text.contains("... 1 more findings (all of them: --json)"),
            "{text}"
        );
        assert!(
            text.contains("3 findings in 1 file, 0 failed steps (hk"),
            "{text}"
        );
    }

    #[test]
    fn paths_are_made_relative_to_the_root() {
        let root = Path::new("/repo");
        assert_eq!(relative(root, "/repo/src/a.rs"), "src/a.rs");
        assert_eq!(relative(root, "./src/a.rs"), "src/a.rs");
        assert_eq!(relative(root, "/elsewhere/a.rs"), "/elsewhere/a.rs");
        assert!(resolve(root, Path::new("/repo/src"), &["../../x".into()]).is_err());
        assert_eq!(
            resolve(
                root,
                Path::new("/repo/src"),
                &["a.rs".into(), "./a.rs".into()]
            )
            .unwrap(),
            ["src/a.rs"]
        );
    }
}
