//! Reading the hook ledger from where the last reader stopped.
//!
//! The ledger writes `ledger.jsonl` and renames it to `ledger-<ms>.jsonl`
//! when it grows too large, so a byte offset alone would point into the
//! wrong file after a rotation. The cursor also keeps the file's first line:
//! whichever file still starts with that line is the one the offset belongs
//! to, and every file after it is new.

use crate::ledger::Entry;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cursor {
    /// The first line of the file the offset is in.
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub offset: u64,
}

/// The ledger's files, oldest first: rotated ones by their timestamp, then
/// the current one.
fn files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rotated: Vec<(u128, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let stamp = path
                .file_name()?
                .to_str()?
                .strip_prefix("ledger-")?
                .strip_suffix(".jsonl")?
                .parse()
                .ok()?;
            Some((stamp, path))
        })
        .collect();
    rotated.sort();
    let mut out: Vec<PathBuf> = rotated.into_iter().map(|(_, p)| p).collect();
    let current = dir.join("ledger.jsonl");
    if current.is_file() {
        out.push(current);
    }
    out
}

fn first_line(path: &Path) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut line = String::new();
    let _ = std::io::BufReader::new(file).read_line(&mut line);
    line
}

/// Every whole line after `start`, parsed, and the offset after the last.
fn read_from(path: &Path, start: u64, entries: &mut Vec<Entry>) -> u64 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return start;
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return start;
    }
    let mut reader = std::io::BufReader::new(file);
    let mut offset = start;
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            // A line without its newline is still being written.
            Ok(_) if line.last() != Some(&b'\n') => break,
            Ok(n) => {
                offset += n as u64;
                if let Ok(entry) = serde_json::from_slice::<Entry>(&line) {
                    entries.push(entry);
                }
            }
        }
    }
    offset
}

/// Every entry after the cursor, and the cursor after the last whole line.
pub(crate) fn read_since(dir: &Path, cursor: &Cursor) -> (Vec<Entry>, Cursor) {
    let files = files(dir);
    let heads: Vec<String> = files.iter().map(|f| first_line(f)).collect();
    // The file the cursor is in; if none still starts with its line (the
    // file was pruned, or there is no cursor yet), read everything.
    let at = heads
        .iter()
        .position(|h| !cursor.head.is_empty() && *h == cursor.head);
    let mut entries = Vec::new();
    let mut next = cursor.clone();
    for (i, (path, head)) in files.iter().zip(heads).enumerate() {
        let start = match at {
            Some(a) if i < a => continue,
            Some(a) if i == a => cursor.offset,
            _ => 0,
        };
        let offset = read_from(path, start, &mut entries);
        if !head.is_empty() {
            next = Cursor { head, offset };
        }
    }
    (entries, next)
}

/// A ledger shape as a script the shell parser reads the way it read the
/// command: its placeholders (`<arg>`, `--flag=<v>`, `<redacted>`) become
/// plain words, where `<` would otherwise read as a redirection.
pub(crate) fn script(shape: &str) -> String {
    shape
        .replace("<arg>", "ARG")
        .replace("<v>", "V")
        .replace("<redacted>", "REDACTED")
}

/// Every entry the ledger holds, oldest first.
pub(crate) fn read_all(dir: &Path) -> Vec<Entry> {
    read_since(dir, &Cursor::default()).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(ts: u64) -> String {
        format!("{{\"ts\":{ts},\"harness\":\"claude\",\"event\":\"stop\"}}\n")
    }

    fn append(path: &Path, text: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn stamps(entries: &[Entry]) -> Vec<u64> {
        entries.iter().map(|e| e.ts).collect()
    }

    #[test]
    fn shapes_become_scripts() {
        assert_eq!(
            script("git commit -m <arg> --author=<v> && echo <redacted>"),
            "git commit -m ARG --author=V && echo REDACTED"
        );
    }

    #[test]
    fn follows_the_ledger_across_a_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("ledger.jsonl");
        append(&current, &(line(1) + &line(2)));
        let (first, cursor) = read_since(dir.path(), &Cursor::default());
        assert_eq!(stamps(&first), [1, 2]);

        // More lines, then a rotation, then a new current file.
        append(&current, &line(3));
        std::fs::rename(&current, dir.path().join("ledger-1000.jsonl")).unwrap();
        append(&current, &line(4));
        let (second, cursor) = read_since(dir.path(), &cursor);
        assert_eq!(stamps(&second), [3, 4]);
        assert!(read_since(dir.path(), &cursor).0.is_empty());

        append(&current, "{\"ts\":5,");
        let (partial, cursor) = read_since(dir.path(), &cursor);
        assert!(partial.is_empty());
        append(&current, "\"harness\":\"h\",\"event\":\"stop\"}\n");
        assert_eq!(stamps(&read_since(dir.path(), &cursor).0), [5]);
        assert_eq!(stamps(&read_all(dir.path())), [1, 2, 3, 4, 5]);
    }
}
