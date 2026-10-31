//! Formatting the files an edit wrote, with the project's own fixers:
//! `hk run fix --safe` on exactly those files.
//!
//! `--safe` makes hk refuse, before anything runs, a fixer whose effect is
//! unknown or destructive, so a fix never runs a command hk cannot vouch
//! for. Which files changed is read from their hashes before and after, not
//! from hk.

use super::{hk_config, hk_program, nul_list, process};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FixStatus {
    /// hk ran; `formatted` lists what it changed, possibly nothing.
    Ran,
    /// No hk config: nothing to run.
    NoConfig,
    /// hk refused (`--safe`) or could not load its config; nothing ran.
    Refused,
    /// hk ran past the deadline and was stopped.
    TimedOut,
    /// hk could not be started.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Fixed {
    pub status: FixStatus,
    /// The files hk changed, relative to the root.
    pub formatted: Vec<String>,
    /// Steps whose fix failed: what is left for the agent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_steps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub duration_ms: u64,
}

impl Fixed {
    /// One line for an agent: what formatting did, or why it did not.
    pub(crate) fn summary(&self) -> Option<String> {
        let n = self.formatted.len();
        match self.status {
            FixStatus::NoConfig => None,
            FixStatus::Ran if n == 0 && self.failed_steps.is_empty() => None,
            FixStatus::Ran => {
                let mut line = format!("hk fix formatted {n} {}", super::plural(n, "file"));
                if n > 0 {
                    line.push_str(&format!(" ({})", self.formatted.join(", ")));
                    line.push_str("; reread before editing them again");
                }
                if !self.failed_steps.is_empty() {
                    line.push_str(&format!(
                        "; failed: {} (run `jouet diag check` on the files)",
                        self.failed_steps.join(", ")
                    ));
                }
                Some(line)
            }
            FixStatus::Refused => Some(format!(
                "hk fix did not run: {}",
                self.reason.as_deref().unwrap_or("refused")
            )),
            FixStatus::TimedOut => Some(format!(
                "hk fix ran past its time budget and was stopped{}",
                if n > 0 {
                    format!(" after changing {}", self.formatted.join(", "))
                } else {
                    String::new()
                }
            )),
            FixStatus::Failed => Some(format!(
                "hk fix could not run: {}",
                self.reason.as_deref().unwrap_or("unknown error")
            )),
        }
    }
}

/// Runs the fixers on `files` (relative to `root`), only `steps` when any
/// are given.
pub(crate) fn run(root: &Path, files: &[String], steps: &[String], timeout: Duration) -> Fixed {
    let started = std::time::Instant::now();
    let mut fixed = Fixed {
        status: FixStatus::NoConfig,
        formatted: Vec::new(),
        failed_steps: Vec::new(),
        reason: None,
        duration_ms: 0,
    };
    if hk_config(root).is_none() || files.is_empty() {
        return fixed;
    }
    let before: Vec<Option<String>> = files.iter().map(|f| super::delta::hash(root, f)).collect();
    let mut argv = vec![hk_program()];
    argv.extend(
        [
            "run",
            "fix",
            "--safe",
            "--no-fail-fast",
            "--stash",
            "none",
            "--no-stage",
            "--format",
            "json",
            "--files0-from",
            "-",
        ]
        .iter()
        .map(ToString::to_string),
    );
    for step in steps {
        argv.push("--step".into());
        argv.push(step.clone());
    }
    let ran = process::run(&argv, root, &nul_list(files), timeout);
    fixed.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    fixed.formatted = files
        .iter()
        .zip(&before)
        .filter(|(f, h)| super::delta::hash(root, f) != **h)
        .map(|(f, _)| f.clone())
        .collect();
    let ran = match ran {
        Ok(ran) => ran,
        Err(e) => {
            fixed.status = FixStatus::Failed;
            fixed.reason = Some(format!("running {}: {e}", argv[0]));
            return fixed;
        }
    };
    if ran.timed_out {
        fixed.status = FixStatus::TimedOut;
        return fixed;
    }
    let result: serde_json::Value = serde_json::from_slice(&ran.stdout).unwrap_or_default();
    let steps = result["steps"].as_array().map_or(&[][..], Vec::as_slice);
    if steps.is_empty() && result["status"] == "failed" {
        fixed.status = FixStatus::Refused;
        fixed.reason = result["failure"].as_str().map(|s| s.trim().to_string());
        return fixed;
    }
    if result.is_null() {
        fixed.status = FixStatus::Failed;
        fixed.reason = Some(format!(
            "hk printed no JSON result: {}",
            String::from_utf8_lossy(&ran.stderr).trim()
        ));
        return fixed;
    }
    fixed.status = FixStatus::Ran;
    fixed.failed_steps = steps
        .iter()
        .filter(|s| s["status"] == "failed")
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_nothing_when_nothing_changed() {
        let mut fixed = Fixed {
            status: FixStatus::Ran,
            formatted: Vec::new(),
            failed_steps: Vec::new(),
            reason: None,
            duration_ms: 3,
        };
        assert_eq!(fixed.summary(), None);
        fixed.formatted = vec!["src/a.rs".into()];
        assert_eq!(
            fixed.summary().unwrap(),
            "hk fix formatted 1 file (src/a.rs); reread before editing them again"
        );
        fixed.status = FixStatus::Refused;
        fixed.reason = Some("--safe refused to run".into());
        assert_eq!(
            fixed.summary().unwrap(),
            "hk fix did not run: --safe refused to run"
        );
    }
}
