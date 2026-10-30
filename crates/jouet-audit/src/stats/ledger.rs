//! jouet's hook ledger: one JSON line per hook event under
//! `$XDG_STATE_HOME/jouet/ledger/`, in `ledger.jsonl` and rotated
//! `ledger-<ms>.jsonl` files.
//!
//! Read here on its own terms rather than through the hooks' code: fields
//! this build does not know are ignored, and a line that is not an entry is
//! skipped and counted.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// One hook event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerEntry {
    /// Seconds since the Unix epoch.
    pub ts: u64,
    /// `claude` or `codex`.
    pub harness: String,
    pub session: Option<String>,
    /// `session-start`, `pre-tool-use`, `post-tool-use`, `stop` or `override`.
    pub event: String,
    pub tool: Option<String>,
    /// The command's shape: known subcommands and flag names, values replaced.
    pub shape: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub decisions: Vec<LedgerDecision>,
}

/// What a rule decided about a call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerDecision {
    pub rule: String,
    /// `deny`, `warn`, `record:<kind>`, `override` or `block`.
    pub action: String,
}

/// Every entry of a ledger directory, oldest first.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub dir: PathBuf,
    pub files: u64,
    pub entries: Vec<LedgerEntry>,
    /// Lines that were not entries, skipped.
    pub bad_lines: u64,
}

impl Ledger {
    /// Reads the ledger in `dir`, or `None` when there is none there.
    pub fn read(dir: &Path) -> io::Result<Option<Self>> {
        let names = match std::fs::read_dir(dir) {
            Ok(names) => names,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut paths = Vec::new();
        for entry in names {
            let path = entry?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if name.is_some_and(|n| {
                n == "ledger.jsonl" || n.starts_with("ledger-") && n.ends_with(".jsonl")
            }) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return Ok(None);
        }
        paths.sort();
        let mut ledger = Self {
            dir: dir.to_path_buf(),
            ..Self::default()
        };
        for path in paths {
            let text = match std::fs::read(&path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                // Rotated away between listing and reading.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            ledger.files += 1;
            ledger.add_lines(&text);
        }
        ledger.entries.sort_by_key(|e| e.ts);
        Ok(Some(ledger))
    }

    /// Parses ledger lines, as from one file.
    pub fn add_lines(&mut self, text: &str) {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<LedgerEntry>(line) {
                Ok(entry) if !entry.event.is_empty() => self.entries.push(entry),
                _ => self.bad_lines += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_and_rotated_files_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ledger.jsonl"),
            "{\"ts\":30,\"harness\":\"claude\",\"event\":\"stop\"}\nnot json\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("ledger-1.jsonl"),
            "{\"ts\":10,\"harness\":\"claude\",\"event\":\"session-start\",\"new_field\":1}\n{\"ts\":20}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let ledger = Ledger::read(dir.path()).unwrap().unwrap();
        assert_eq!(ledger.files, 2);
        assert_eq!(ledger.bad_lines, 2);
        let ts: Vec<u64> = ledger.entries.iter().map(|e| e.ts).collect();
        assert_eq!(ts, [10, 30]);
    }

    #[test]
    fn no_directory_or_no_files_is_no_ledger() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Ledger::read(&dir.path().join("missing")).unwrap().is_none());
        assert!(Ledger::read(dir.path()).unwrap().is_none());
    }
}
