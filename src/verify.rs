//! The check before an agent stops: if the last test, lint or build run in
//! this session failed, the stop hook blocks once and says which.
//!
//! What counts as a check is decided by post-tool-use rules (the built-in
//! `checks` rule, or a user's replacement), which `jouet.record` the runs
//! they recognise. Claude Code reports each command's exit code to a hook as
//! it happens (PostToolUse on success, PostToolUseFailure with `Exit code N`),
//! and those records are kept in a small per-session state file. Codex sends
//! no exit code to hooks, so its commands are read from the end of the
//! session transcript when it stops, and run through the same rules then.

use jouet_rules::{Kind, Record};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub command: String,
    pub exit_code: i32,
}

/// The latest run of each kind of check.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub runs: BTreeMap<Kind, Run>,
}

impl State {
    pub fn record(&mut self, records: impl IntoIterator<Item = Record>) {
        for record in records {
            self.runs.insert(
                record.kind,
                Run {
                    command: record.command,
                    exit_code: record.exit_code,
                },
            );
        }
    }

    /// Why stopping should wait, if the last run of any check failed.
    pub fn block_reason(&self) -> Option<String> {
        let (kind, run) = self.runs.iter().find(|(_, run)| run.exit_code != 0)?;
        Some(format!(
            "The last {} run failed (`{}` exited {}). Fix it and run it again before you finish, or say plainly why it cannot pass. [jouet: verify]",
            kind.name(),
            run.command,
            run.exit_code
        ))
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut file, self)?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

/// Loads, changes and saves a session's state under an exclusive lock, so
/// two hooks finishing at once cannot each save a copy without the other's
/// run.
pub fn update(path: &Path, change: impl FnOnce(&mut State)) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    // Released when `lock` is dropped.
    lock.lock()?;
    let mut state = State::load(path);
    change(&mut state);
    state.save(path)
}

/// The state file for a session, under `$XDG_STATE_HOME/jouet/sessions/`.
pub fn state_path(session_id: &str) -> Option<PathBuf> {
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    (!safe.is_empty()).then(|| sessions_dir().join(format!("{safe}.json")))
}

pub fn sessions_dir() -> PathBuf {
    crate::paths::state_dir().join("sessions")
}

/// Removes session files untouched for a week. Best effort.
pub fn prune_sessions() {
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else {
        return;
    };
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// How much of a Codex transcript is read, from the end.
const TRANSCRIPT_TAIL: u64 = 8 * 1024 * 1024;

/// The finished commands at the end of a Codex session transcript, oldest
/// first, with their exit codes.
///
/// The transcript format is Codex's own and undocumented; lines this does not
/// recognise are skipped, so a format change means no blocking, not false
/// blocking.
pub fn codex_commands(path: &Path) -> Vec<(String, i32)> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        // Starts mid-line.
        lines.next();
    }
    lines
        .filter(|line| line.contains("CommandExecution") || line.contains("exec_command_end"))
        .filter_map(codex_command)
        .collect()
}

/// A finished command from one transcript line: `item_completed` events for
/// a `CommandExecution` item, or older `exec_command_end` events.
fn codex_command(line: &str) -> Option<(String, i32)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = value.get("payload")?;
    let event = match payload.get("type")?.as_str()? {
        "item_completed" => payload
            .get("item")
            .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("CommandExecution"))?,
        "exec_command_end" => payload,
        _ => return None,
    };
    let exit_code = i32::try_from(event.get("exit_code")?.as_i64()?).ok()?;
    let command: Vec<&str> = event
        .get("command")?
        .as_array()?
        .iter()
        .filter_map(|w| w.as_str())
        .collect();
    // `["/usr/bin/zsh", "-lc", "cargo test"]`: the script is the last word.
    let script = match command.as_slice() {
        [_, flag, script] if flag.starts_with('-') && flag.contains('c') => script.to_string(),
        words => words.join(" "),
    };
    Some((script, exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: Kind, command: &str, exit_code: i32) -> Record {
        Record {
            rule: "checks".into(),
            kind,
            command: command.into(),
            exit_code,
        }
    }

    #[test]
    fn blocks_only_while_the_last_run_failed() {
        let mut state = State::default();
        state.record([record(Kind::Test, "cargo test", 101)]);
        let reason = state.block_reason().unwrap();
        assert!(reason.contains("`cargo test` exited 101"), "{reason}");
        state.record([record(Kind::Test, "cargo test -p jouet", 0)]);
        assert_eq!(state.block_reason(), None);
    }

    #[test]
    fn state_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let mut state = State::default();
        state.record([record(Kind::Lint, "cargo clippy", 101)]);
        state.save(&path).unwrap();
        assert_eq!(State::load(&path), state);
        assert_eq!(
            State::load(&dir.path().join("missing.json")),
            State::default()
        );
    }

    #[test]
    fn concurrent_updates_keep_every_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let threads: Vec<_> = [(Kind::Test, 101), (Kind::Lint, 0), (Kind::Build, 1)]
            .into_iter()
            .map(|(kind, code)| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..40 {
                        let run = record(kind, &format!("run {i}"), code);
                        update(&path, |s| s.record([run])).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let state = State::load(&path);
        assert_eq!(state.runs.len(), 3, "{state:?}");
        assert_eq!(state.runs[&Kind::Test].exit_code, 101);
    }

    #[test]
    fn session_ids_cannot_escape_the_state_directory() {
        let path = state_path("../../etc/passwd").unwrap();
        assert_eq!(path.file_name().unwrap(), "etcpasswd.json");
        assert_eq!(state_path("../"), None);
    }

    #[test]
    fn reads_commands_from_a_codex_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let item = |cmd: &str, code: i32| {
            serde_json::json!({"type": "event_msg", "payload": {"type": "item_completed",
                "item": {"type": "CommandExecution", "command": ["/usr/bin/zsh", "-lc", cmd],
                    "status": "completed", "exit_code": code}}})
            .to_string()
        };
        let lines = [
            item("cargo test", 101),
            r#"{"type":"response_item","payload":{"type":"message"}}"#.to_string(),
            "not json, but mentions CommandExecution".to_string(),
            r#"{"type":"event_msg","payload":{"type":"exec_command_end","command":["ls"],"exit_code":0}}"#.to_string(),
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        assert_eq!(
            codex_commands(&path),
            [("cargo test".to_string(), 101), ("ls".to_string(), 0)]
        );
        assert!(codex_commands(&dir.path().join("missing")).is_empty());
    }
}
