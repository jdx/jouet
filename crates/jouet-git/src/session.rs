//! The last test, lint and build runs of an agent session, as `jouet
//! setup`'s hooks record them in `$XDG_STATE_HOME/jouet/sessions/ID.json`:
//! `{"runs": {"test": {"command": "cargo test", "exit_code": 0}}}`.
//!
//! Reading is best effort: no session ID, no file, or a file in a shape
//! this build does not know all mean nothing was recorded.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
    /// `test`, `lint` or `build`.
    pub kind: String,
    pub command: String,
    pub exit_code: i32,
}

#[derive(Deserialize)]
struct State {
    #[serde(default)]
    runs: BTreeMap<String, Run>,
}

#[derive(Deserialize)]
struct Run {
    command: String,
    exit_code: i32,
}

/// The session ID to read: `JOUET_SESSION_ID`, else Claude Code's
/// `CLAUDE_CODE_SESSION_ID`.
pub fn id_from_env() -> Option<String> {
    ["JOUET_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.trim().is_empty())
}

/// The file the hooks keep for a session, named the way they name it.
fn path(dir: &Path, id: &str) -> Option<PathBuf> {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    (!safe.is_empty()).then(|| dir.join(format!("{safe}.json")))
}

/// The session's latest run of each kind of check, or `None` when nothing
/// was recorded.
pub fn last_checks(dir: &Path, id: &str) -> Option<Vec<CheckRun>> {
    let bytes = std::fs::read(path(dir, id)?).ok()?;
    let state: State = serde_json::from_slice(&bytes).ok()?;
    let runs: Vec<CheckRun> = state
        .runs
        .into_iter()
        .map(|(kind, run)| CheckRun {
            kind,
            command: run.command,
            exit_code: run.exit_code,
        })
        .collect();
    (!runs.is_empty()).then_some(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_hooks_record() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("abc-123.json"),
            r#"{"runs":{"test":{"command":"cargo test","exit_code":101},"lint":{"command":"mise run lint","exit_code":0}}}"#,
        )
        .unwrap();
        let runs = last_checks(dir.path(), "abc-123").unwrap();
        assert_eq!(
            runs,
            [
                CheckRun {
                    kind: "lint".into(),
                    command: "mise run lint".into(),
                    exit_code: 0
                },
                CheckRun {
                    kind: "test".into(),
                    command: "cargo test".into(),
                    exit_code: 101
                },
            ]
        );
        assert_eq!(last_checks(dir.path(), "other"), None);
        assert_eq!(last_checks(dir.path(), "../../etc"), None);
        std::fs::write(dir.path().join("bad.json"), "not json").unwrap();
        assert_eq!(last_checks(dir.path(), "bad"), None);
        std::fs::write(dir.path().join("empty.json"), r#"{"runs":{}}"#).unwrap();
        assert_eq!(last_checks(dir.path(), "empty"), None);
    }
}
