//! What one agent session has already been shown: for each file, the lines
//! shown and the file's size and modification time when they were.
//!
//! A file whose size or modification time has changed since is treated as
//! unseen. Kept in `<state dir>/seen/<session>.json`. Reading and writing are
//! best effort: a missing or unreadable file is an empty memory, and a
//! failed write loses only what this command showed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Session files untouched for this long are removed.
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A file's size and modification time, which say whether it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub len: u64,
    /// Nanoseconds since the epoch.
    pub mtime: u128,
}

impl Stamp {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            mtime: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos()),
        }
    }

    pub fn read(path: &Path) -> Option<Self> {
        std::fs::metadata(path).ok().map(|m| Self::of(&m))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    stamp: Stamp,
    /// Inclusive 1-based line ranges, sorted and merged.
    lines: Vec<(u32, u32)>,
}

impl Seen {
    fn covers(&self, line: u32) -> bool {
        let i = self.lines.partition_point(|&(_, end)| end < line);
        self.lines.get(i).is_some_and(|&(start, _)| start <= line)
    }

    fn add(&mut self, start: u32, end: u32) {
        self.lines.push((start, end));
        self.lines.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.lines.len());
        for &(s, e) in &self.lines {
            match merged.last_mut() {
                Some(last) if s <= last.1.saturating_add(1) => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.lines = merged;
    }
}

/// One session's memory.
#[derive(Debug, Default)]
pub struct Memory {
    path: Option<PathBuf>,
    files: HashMap<String, Seen>,
    dirty: bool,
}

#[derive(Serialize, Deserialize)]
struct Doc {
    files: HashMap<String, Seen>,
}

impl Memory {
    /// A memory that remembers nothing and saves nowhere, for a command run
    /// outside a session or with `--again`.
    pub fn none() -> Self {
        Self::default()
    }

    /// The memory of session `id`, kept under `dir`.
    pub fn open(dir: &Path, id: &str) -> Self {
        let Some(path) = file(dir, id) else {
            return Self::none();
        };
        let files = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Doc>(&bytes).ok())
            .map(|doc| doc.files)
            .unwrap_or_default();
        Self {
            path: Some(path),
            files,
            dirty: false,
        }
    }

    /// Whether this memory belongs to a session.
    pub fn active(&self) -> bool {
        self.path.is_some()
    }

    /// Whether `line` of the file `key` was shown while the file was as
    /// `stamp` says it is now.
    pub fn shown(&self, key: &str, stamp: Stamp, line: u32) -> bool {
        self.files
            .get(key)
            .is_some_and(|seen| seen.stamp == stamp && seen.covers(line))
    }

    /// Whether every line from `start` to `end` was shown, unchanged.
    pub fn shown_range(&self, key: &str, stamp: Stamp, start: u32, end: u32) -> bool {
        self.files.get(key).is_some_and(|seen| {
            seen.stamp == stamp && {
                let i = seen.lines.partition_point(|&(_, e)| e < start);
                seen.lines
                    .get(i)
                    .is_some_and(|&(s, e)| s <= start && end <= e)
            }
        })
    }

    /// Records that lines `start..=end` of the file `key` were shown. A file
    /// that changed since it was last recorded starts again.
    pub fn record(&mut self, key: &str, stamp: Stamp, start: u32, end: u32) {
        if !self.active() {
            return;
        }
        let seen = self.files.entry(key.to_string()).or_insert(Seen {
            stamp,
            lines: Vec::new(),
        });
        if seen.stamp != stamp {
            seen.stamp = stamp;
            seen.lines.clear();
        }
        seen.add(start.min(end), end.max(start));
        self.dirty = true;
    }

    /// Writes the memory back, replacing the file whole, and removes other
    /// sessions' files that have not been touched for a week.
    pub fn save(&self) {
        let Some(path) = self.path.as_deref().filter(|_| self.dirty) else {
            return;
        };
        let _ = write(path, &self.files);
        if let Some(dir) = path.parent() {
            prune(dir);
        }
    }
}

/// The key a file is remembered by: its canonical path, so a file reached
/// from another directory or through `./` is the same file.
pub fn key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// The file a session's memory is kept in, named the way the hook ledger
/// names sessions.
fn file(dir: &Path, id: &str) -> Option<PathBuf> {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    (!safe.is_empty()).then(|| dir.join(format!("{safe}.json")))
}

fn write(path: &Path, files: &HashMap<String, Seen>) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut tmp, &DocRef { files })?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[derive(Serialize)]
struct DocRef<'a> {
    files: &'a HashMap<String, Seen>,
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > KEEP);
        if old && entry.path().extension().is_some_and(|e| e == "json") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: Stamp = Stamp { len: 10, mtime: 1 };

    #[test]
    fn ranges_merge_and_cover() {
        let mut seen = Seen {
            stamp: STAMP,
            lines: Vec::new(),
        };
        seen.add(10, 12);
        seen.add(1, 3);
        seen.add(4, 4);
        seen.add(11, 20);
        assert_eq!(seen.lines, vec![(1, 4), (10, 20)]);
        assert!(seen.covers(1) && seen.covers(4) && seen.covers(15));
        assert!(!seen.covers(5) && !seen.covers(9) && !seen.covers(21));
    }

    #[test]
    fn a_changed_file_is_unseen_and_starts_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut memory = Memory::open(dir.path(), "session-1");
        memory.record("a.rs", STAMP, 5, 5);
        assert!(memory.shown("a.rs", STAMP, 5));
        let changed = Stamp { len: 11, mtime: 2 };
        assert!(!memory.shown("a.rs", changed, 5));
        memory.record("a.rs", changed, 7, 7);
        assert!(!memory.shown("a.rs", changed, 5));
        assert!(memory.shown("a.rs", changed, 7));
    }

    #[test]
    fn memory_survives_a_save_and_is_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut memory = Memory::open(dir.path(), "s/../1");
        memory.record("a.rs", STAMP, 1, 3);
        memory.save();
        assert!(dir.path().join("s1.json").exists());
        let again = Memory::open(dir.path(), "s/../1");
        assert!(again.shown_range("a.rs", STAMP, 2, 3));
        assert!(!again.shown_range("a.rs", STAMP, 2, 4));
        assert!(!Memory::open(dir.path(), "other").shown("a.rs", STAMP, 1));
    }

    #[test]
    fn no_session_remembers_nothing() {
        let mut memory = Memory::none();
        memory.record("a.rs", STAMP, 1, 1);
        assert!(!memory.shown("a.rs", STAMP, 1));
        memory.save();
    }
}
