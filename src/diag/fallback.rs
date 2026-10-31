//! Without an hk config: the language's own checker, in its JSON mode.
//!
//! Deliberately small. Rust files get `cargo clippy --message-format=json`
//! (the crate, since clippy cannot check one file), Python files `ruff check
//! --output-format=json`, and shell scripts `shellcheck --format=json1`,
//! each only when the tool is on `PATH`. Any other file has no checker, and
//! the report says diagnostics are unavailable for it.

use super::{Finding, Report, Status, Step, process, relative, save_log};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

struct Checker {
    name: &'static str,
    program: &'static str,
    extensions: &'static [&'static str],
}

const CHECKERS: &[Checker] = &[
    Checker {
        name: "cargo clippy",
        program: "cargo",
        extensions: &["rs"],
    },
    Checker {
        name: "ruff",
        program: "ruff",
        extensions: &["py", "pyi"],
    },
    Checker {
        name: "shellcheck",
        program: "shellcheck",
        extensions: &["sh", "bash"],
    },
];

pub(super) fn check(root: &Path, files: Vec<String>, timeout: Duration) -> Report {
    let mut report = Report::empty(Status::Findings, files);
    let mut uncovered = Vec::new();
    let mut ran = Vec::new();
    for checker in CHECKERS {
        let mine: Vec<&String> = report
            .files
            .iter()
            .filter(|f| {
                Path::new(f)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| checker.extensions.contains(&e))
            })
            .collect();
        if mine.is_empty() {
            continue;
        }
        if !on_path(checker.program)
            || (checker.program == "cargo" && !root.join("Cargo.toml").is_file())
        {
            uncovered.extend(mine.iter().map(|f| (*f).clone()));
            continue;
        }
        let mine: Vec<String> = mine.into_iter().cloned().collect();
        let (findings, status, output) = run(checker, root, &mine, timeout);
        ran.push(checker.name);
        let excerpt = (status == "failed" && findings.is_empty())
            .then(|| super::excerpt(&output, None))
            .flatten();
        report.steps.push(Step {
            name: checker.name.to_string(),
            status: status.to_string(),
            findings: findings.len(),
            excerpt,
        });
        report.findings.extend(findings);
        report.log_path = save_log("diag-fallback", output.as_bytes()).or(report.log_path);
    }
    for file in &report.files {
        let checked = CHECKERS.iter().any(|c| {
            ran.contains(&c.name)
                && Path::new(file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| c.extensions.contains(&e))
        });
        if !checked && !uncovered.contains(file) {
            uncovered.push(file.clone());
        }
    }
    if ran.is_empty() {
        report.status = Status::Unavailable;
        report.reason = Some(format!(
            "no hk config (hk.pkl) here, and no fallback checker for {}: jouet falls back to cargo clippy, ruff and shellcheck when they are on PATH",
            uncovered.join(", ")
        ));
        return report;
    }
    report.source = ran.join(", ");
    if !uncovered.is_empty() {
        report.reason = Some(format!(
            "no hk config, and no fallback checker for {}",
            uncovered.join(", ")
        ));
    }
    report
}

/// Runs one checker and returns its findings, `passed` or `failed`, and its
/// output.
fn run(
    checker: &Checker,
    root: &Path,
    files: &[String],
    timeout: Duration,
) -> (Vec<Finding>, &'static str, String) {
    let argv: Vec<String> = match checker.program {
        "cargo" => vec![
            "cargo".into(),
            "clippy".into(),
            "--quiet".into(),
            "--message-format=json".into(),
        ],
        "ruff" => ["ruff", "check", "--output-format=json", "--"]
            .iter()
            .map(ToString::to_string)
            .chain(files.iter().cloned())
            .collect(),
        _ => ["shellcheck", "--format=json1", "--"]
            .iter()
            .map(ToString::to_string)
            .chain(files.iter().cloned())
            .collect(),
    };
    let ran = match process::run(&argv, root, b"", timeout) {
        Ok(ran) => ran,
        Err(e) => return (Vec::new(), "failed", format!("running {}: {e}", argv[0])),
    };
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    let findings = match checker.program {
        "cargo" => cargo(root, &stdout, checker.name),
        "ruff" => ruff(root, &stdout, checker.name),
        _ => shellcheck(root, &stdout, checker.name),
    };
    let status = if ran.code == Some(0) && !ran.timed_out {
        "passed"
    } else {
        "failed"
    };
    let output = if findings.is_empty() {
        format!("{stdout}{}", String::from_utf8_lossy(&ran.stderr))
    } else {
        stdout
    };
    (findings, status, output)
}

/// `cargo clippy --message-format=json`: each `compiler-message` with a
/// primary span.
fn cargo(root: &Path, stdout: &str, step: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-message" {
            continue;
        }
        let message = &value["message"];
        let Some(span) = message["spans"]
            .as_array()
            .and_then(|spans| spans.iter().find(|s| s["is_primary"] == true))
        else {
            continue;
        };
        // Spans are relative to the workspace root, which is `root` here.
        let file = span["file_name"]
            .as_str()
            .map_or_else(String::new, |p| relative(root, p));
        let finding = Finding {
            file,
            line: span["line_start"].as_u64(),
            col: span["column_start"].as_u64(),
            severity: match message["level"].as_str() {
                Some("warning") => "warning",
                Some("note") => "note",
                Some("help") => "help",
                _ => "error",
            }
            .into(),
            rule: message["code"]["code"].as_str().map(str::to_string),
            message: message["message"].as_str().unwrap_or("").to_string(),
            source_step: step.into(),
        };
        if !out.contains(&finding) {
            out.push(finding);
        }
    }
    out
}

/// `ruff check --output-format=json`: an array of findings.
fn ruff(root: &Path, stdout: &str, step: &str) -> Vec<Finding> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|i| Finding {
            file: i["filename"]
                .as_str()
                .map_or_else(String::new, |p| relative(root, p)),
            line: i["location"]["row"].as_u64(),
            col: i["location"]["column"].as_u64(),
            severity: "error".into(),
            rule: i["code"].as_str().map(str::to_string),
            message: i["message"].as_str().unwrap_or("").to_string(),
            source_step: step.into(),
        })
        .collect()
}

/// `shellcheck --format=json1`: `{"comments": [...]}`.
fn shellcheck(root: &Path, stdout: &str, step: &str) -> Vec<Finding> {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    value["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Finding {
            file: c["file"]
                .as_str()
                .map_or_else(String::new, |p| relative(root, p)),
            line: c["line"].as_u64(),
            col: c["column"].as_u64(),
            severity: match c["level"].as_str() {
                Some("warning") => "warning",
                Some("info" | "style") => "note",
                _ => "error",
            }
            .into(),
            rule: c["code"].as_u64().map(|code| format!("SC{code}")),
            message: c["message"].as_str().unwrap_or("").to_string(),
            source_step: step.into(),
        })
        .collect()
}

pub(crate) fn on_path(program: &str) -> bool {
    let names = if cfg!(windows) {
        vec![
            format!("{program}.exe"),
            format!("{program}.cmd"),
            program.to_string(),
        ]
    } else {
        vec![program.to_string()]
    };
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_checkers_json() {
        let root = Path::new("/repo");
        let cargo_out = r#"{"reason":"compiler-artifact"}
{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `x`","code":{"code":"unused_variables"},"spans":[{"file_name":"src/main.rs","is_primary":true,"line_start":2,"column_start":9}]}}
{"reason":"compiler-message","message":{"level":"warning","message":"1 warning emitted","code":null,"spans":[]}}"#;
        let found = cargo(root, cargo_out, "cargo clippy");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "src/main.rs");
        assert_eq!(found[0].rule.as_deref(), Some("unused_variables"));

        let ruff_out = r#"[{"code":"F401","message":"`os` imported but unused","filename":"/repo/t.py","location":{"row":1,"column":8}}]"#;
        let found = ruff(root, ruff_out, "ruff");
        assert_eq!(
            (found[0].file.as_str(), found[0].line, found[0].col),
            ("t.py", Some(1), Some(8))
        );

        let sc = r#"{"comments":[{"file":"a.sh","line":2,"column":6,"level":"info","code":2086,"message":"Double quote"}]}"#;
        let found = shellcheck(root, sc, "shellcheck");
        assert_eq!(found[0].rule.as_deref(), Some("SC2086"));
        assert_eq!(found[0].severity, "note");
    }
}
