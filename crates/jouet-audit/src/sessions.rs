//! Which agent sessions wrote to their transcripts lately, and where they
//! ran, for `jouet sessions`.
//!
//! Only transcripts written within the window are opened, and of each only
//! a little is read: the end of a Claude Code transcript for the latest
//! line with a `cwd` (and its `gitBranch`), and a Codex rollout's first
//! line, `session_meta`, with the end for the latest `turn_context`'s `cwd`.
//! Nothing a session said is read into the result, and nothing is written.

use crate::read::{ClaudeCode, Codex, Source, files_under};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How much of the end of a transcript is read.
const TAIL: u64 = 64 * 1024;
/// The longest first line of a Codex rollout that is read: its
/// `session_meta` carries the base instructions.
const MAX_META: u64 = 1024 * 1024;

/// One session, as its transcript last recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// `claude` or `codex`.
    pub harness: &'static str,
    pub id: String,
    /// The directory the session last worked in.
    pub cwd: Option<String>,
    /// The branch the transcript recorded: Claude Code's latest
    /// `gitBranch`, or the one Codex started on.
    pub branch: Option<String>,
    /// When the transcript was last written, in milliseconds since the
    /// Unix epoch.
    pub modified_ms: i64,
    pub transcript: PathBuf,
}

/// Every session of either harness whose transcript was written at or
/// after `since_ms`, newest first.
pub fn recent(claude: Option<&ClaudeCode>, codex: Option<&Codex>, since_ms: i64) -> Vec<Session> {
    let mut out = Vec::new();
    if let Some(claude) = claude {
        for source in recent_files(claude_transcripts(&claude.root), since_ms) {
            out.extend(read_claude(&source));
        }
    }
    if let Some(codex) = codex {
        let mut paths = Vec::new();
        let _ = files_under(&codex.root.join("sessions"), &[".jsonl"], &mut paths);
        paths.retain(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
        });
        for source in recent_files(paths, since_ms) {
            out.extend(read_codex(&source));
        }
    }
    out.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// `projects/<project>/<session>.jsonl`, leaving out subagents' transcripts,
/// which live a level below and belong to their session.
fn claude_transcripts(root: &Path) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(root.join("projects")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_file())
                && path.extension().is_some_and(|e| e == "jsonl")
            {
                out.push(path);
            }
        }
    }
    out
}

/// The files last written at or after `since_ms`, judged by their metadata
/// alone.
fn recent_files(paths: Vec<PathBuf>, since_ms: i64) -> Vec<Source> {
    paths
        .into_iter()
        .filter_map(|p| Source::from_path(p).ok())
        .filter(|s| s.modified_ms.is_some_and(|m| m >= since_ms))
        .collect()
}

fn read_claude(source: &Source) -> Option<Session> {
    let id = source.path.file_stem()?.to_string_lossy().into_owned();
    let found = |line: &Value| line.get("cwd").and_then(Value::as_str).is_some();
    let line = last_line(&source.path, b"\"cwd\"", found)
        .or_else(|| first_line(&source.path, TAIL, found))?;
    Some(Session {
        harness: "claude",
        id,
        cwd: text(&line, &["cwd"]),
        branch: text(&line, &["gitBranch"]).filter(|b| !b.is_empty()),
        modified_ms: source.modified_ms?,
        transcript: source.path.clone(),
    })
}

fn read_codex(source: &Source) -> Option<Session> {
    let meta = first_line(&source.path, MAX_META, |line| {
        line.get("type").and_then(Value::as_str) == Some("session_meta")
    })?;
    let turn = last_line(&source.path, b"\"turn_context\"", |line| {
        line.get("type").and_then(Value::as_str) == Some("turn_context")
    });
    let cwd = turn
        .as_ref()
        .and_then(|t| text(t, &["payload", "cwd"]))
        .or_else(|| text(&meta, &["payload", "cwd"]));
    Some(Session {
        harness: "codex",
        id: text(&meta, &["payload", "id"]).or_else(|| text(&meta, &["payload", "session_id"]))?,
        cwd,
        branch: text(&meta, &["payload", "git", "branch"]).filter(|b| !b.is_empty()),
        modified_ms: source.modified_ms?,
        transcript: source.path.clone(),
    })
}

fn text(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |v, key| v.get(key))?
        .as_str()
        .map(str::to_string)
}

/// The last line in the end of the file that holds `needle` and parses to
/// what `wanted` accepts.
fn last_line(path: &Path, needle: &[u8], wanted: impl Fn(&Value) -> bool) -> Option<Value> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if start > 0 && !lines.is_empty() {
        // The read started mid-line.
        lines.remove(0);
    }
    lines
        .into_iter()
        .rev()
        .filter(|line| memchr::memmem::find(line, needle).is_some())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find(|line| wanted(line))
}

/// The first whole line within the first `limit` bytes that parses to
/// what `wanted` accepts.
fn first_line(path: &Path, limit: u64, wanted: impl Fn(&Value) -> bool) -> Option<Value> {
    let mut reader = BufReader::new(File::open(path).ok()?.take(limit));
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) if !line.ends_with(b"\n") => return None,
            Ok(_) => {}
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&line)
            && wanted(&value)
        {
            return Some(value);
        }
    }
}

/// The branch checked out in the work tree at `root`, read from its `HEAD`
/// without running git: the branch name, or the first 12 digits of a
/// detached HEAD.
pub fn head_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let link = std::fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(link.strip_prefix("gitdir:")?.trim());
        if dir.is_absolute() {
            dir
        } else {
            root.join(dir)
        }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(name) => {
            let name = name.trim();
            Some(name.strip_prefix("refs/heads/").unwrap_or(name).to_string())
        }
        None => Some(head.chars().take(12).collect()),
    }
}

#[cfg(test)]
mod tests;
