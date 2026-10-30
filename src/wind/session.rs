//! Finding a harness's session transcripts and digesting them, for
//! `jouet wind digest` and `jouet wind review`.

use jouet_audit::digest::{self, Digest, Options};
use jouet_audit::{ClaudeCode, Codex, Filter, Gitleaks, Reader, Source};
use std::path::{Path, PathBuf};

/// Sessions read for one `--since` at most, the most recent kept.
pub(crate) const MAX_SESSIONS: usize = 8;

/// Which sessions to digest.
pub(crate) enum Select<'a> {
    /// A session ID, a prefix of one, or a transcript's path.
    Named(&'a str),
    /// The most recently written session.
    Last,
    /// Every session written since this time, in milliseconds.
    Since(i64),
}

/// The harnesses' readers, from their own environment variables.
pub(crate) struct Readers {
    claude: Option<ClaudeCode>,
    codex: Option<Codex>,
}

impl Readers {
    pub(crate) fn from_env(harness: Option<&str>) -> Result<Self, String> {
        let (claude, codex) = match harness.unwrap_or("all") {
            "all" => (true, true),
            "claude" => (true, false),
            "codex" => (false, true),
            other => {
                return Err(format!(
                    "--harness must be claude, codex or all, not `{other}`"
                ));
            }
        };
        Ok(Self {
            claude: claude.then(ClaudeCode::from_env).flatten(),
            codex: codex.then(Codex::from_env).flatten(),
        })
    }

    fn all(&self) -> Vec<&dyn Reader> {
        let mut out: Vec<&dyn Reader> = Vec::new();
        if let Some(r) = &self.claude {
            out.push(r);
        }
        if let Some(r) = &self.codex {
            out.push(r);
        }
        out
    }

    /// The reader a transcript's path belongs to: Codex names its files
    /// `rollout-*`, Claude Code by session ID.
    fn for_path(&self, path: &Path) -> Option<&dyn Reader> {
        let name = path.file_name()?.to_string_lossy();
        if name.starts_with("rollout-") {
            self.codex.as_ref().map(|r| r as &dyn Reader)
        } else {
            self.claude.as_ref().map(|r| r as &dyn Reader)
        }
    }
}

/// A transcript chosen to digest.
pub(crate) struct Found<'a> {
    reader: &'a dyn Reader,
    pub source: Source,
}

/// A session's own transcript, not one of its subagents'.
fn main_transcript(source: &Source) -> bool {
    !source
        .path
        .components()
        .any(|c| c.as_os_str() == "subagents")
}

/// The ID a transcript's file is named with: Claude Code's whole stem, the
/// end of a Codex rollout's.
fn id_of(path: &Path) -> String {
    let name = path.file_name().map_or_else(String::new, |n| {
        n.to_string_lossy()
            .trim_end_matches(".zst")
            .trim_end_matches(".jsonl")
            .to_string()
    });
    match name.strip_prefix("rollout-") {
        // rollout-2026-10-30T10-00-00-<uuid>: the UUID is the last five
        // dash-separated groups.
        Some(rest) => {
            let parts: Vec<&str> = rest.split('-').collect();
            parts[parts.len().saturating_sub(5)..].join("-")
        }
        None => name,
    }
}

/// The transcripts `select` names, newest first; `skipped` counts those
/// left out past [`MAX_SESSIONS`].
pub(crate) fn find<'a>(
    readers: &'a Readers,
    select: &Select<'_>,
    project: Option<&Path>,
) -> Result<(Vec<Found<'a>>, usize), String> {
    if let Select::Named(name) = select {
        let path = Path::new(name);
        if path.is_file() {
            let reader = readers
                .for_path(path)
                .ok_or_else(|| format!("no harness reads {name}"))?;
            let source = Source::from_path(path.to_path_buf())
                .map_err(|e| format!("reading {name}: {e}"))?;
            return Ok((vec![Found { reader, source }], 0));
        }
    }
    let filter = Filter {
        project: project.map(Path::to_path_buf),
        since_ms: match select {
            Select::Since(ms) => Some(*ms),
            _ => None,
        },
        ..Filter::default()
    };
    let mut all: Vec<Found<'a>> = Vec::new();
    for reader in readers.all() {
        let sources = reader
            .discover(&filter)
            .map_err(|e| format!("listing {} sessions: {e}", reader.harness()))?;
        all.extend(
            sources
                .into_iter()
                .filter(main_transcript)
                .map(|source| Found { reader, source }),
        );
    }
    all.sort_by_key(|f| std::cmp::Reverse(f.source.modified_ms));
    match select {
        Select::Named(name) => {
            let matching: Vec<Found<'a>> = all
                .into_iter()
                .filter(|f| id_of(&f.source.path).starts_with(name))
                .collect();
            match matching.len() {
                0 => Err(format!("no session `{name}`")),
                1 => Ok((matching, 0)),
                n => {
                    let ids: Vec<String> = matching
                        .iter()
                        .take(5)
                        .map(|f| id_of(&f.source.path))
                        .collect();
                    Err(format!(
                        "`{name}` matches {n} sessions ({}{}); give more of the ID",
                        ids.join(", "),
                        if n > 5 { ", …" } else { "" }
                    ))
                }
            }
        }
        Select::Last => {
            // With --project, the newest transcript may be another
            // project's: read until one matches.
            for found in all {
                if project.is_none() || read(&found, project, true).is_some() {
                    return Ok((vec![found], 0));
                }
            }
            Err("no session to digest".into())
        }
        Select::Since(since) => {
            let mut fresh: Vec<Found<'a>> = all
                .into_iter()
                .filter(|f| f.source.modified_ms.is_none_or(|m| m >= *since))
                // A transcript names its directory in its first lines, so
                // this reads little of one outside the project.
                .filter(|f| project.is_none() || read(f, project, false).is_some())
                .collect();
            let skipped = fresh.len().saturating_sub(MAX_SESSIONS);
            fresh.truncate(MAX_SESSIONS);
            Ok((fresh, skipped))
        }
    }
}

fn read(
    found: &Found<'_>,
    project: Option<&Path>,
    detail: bool,
) -> Option<jouet_audit::Transcript> {
    let filter = Filter {
        project: project.map(Path::to_path_buf),
        detail,
        ..Filter::default()
    };
    let mut input = jouet_audit::open(&found.source.path).ok()?;
    found
        .reader
        .read(&found.source, &mut input, &filter)
        .ok()
        .flatten()
}

/// What removes secrets from a digest: jouet's own patterns, and gitleaks'
/// rules when a copy of its config is at `path`.
pub(crate) struct Redactor {
    gitleaks: Option<Gitleaks>,
    /// Why gitleaks' rules are not used, when a config was named but could
    /// not be read.
    pub problem: Option<String>,
}

impl Redactor {
    /// gitleaks' rules from `configured`, or from
    /// `~/.config/jouet/gitleaks.toml` when that exists.
    pub(crate) fn new(configured: Option<&str>) -> Self {
        let home = jouet_audit::home();
        let expand = |p: &str| match (p.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(p),
        };
        let path = configured.map(expand).or_else(|| {
            jouet_output::paths::config_dir()
                .map(|d| d.join("gitleaks.toml"))
                .filter(|p| p.is_file())
        });
        match path.map(|p| Gitleaks::load(&p)) {
            None => Self {
                gitleaks: None,
                problem: None,
            },
            Some(Ok(g)) => Self {
                gitleaks: Some(g),
                problem: None,
            },
            Some(Err(e)) => Self {
                gitleaks: None,
                problem: Some(e),
            },
        }
    }

    pub(crate) fn redact(&self, text: &str) -> String {
        let text = jouet_audit::redact(text);
        match &self.gitleaks {
            Some(g) => g.redact(&text),
            None => text,
        }
    }

    /// Which rules redacted, for the digest's reader.
    pub(crate) fn describe(&self) -> String {
        match &self.gitleaks {
            Some(g) => format!("jouet's secret patterns and {} gitleaks rules", g.rules()),
            None => "jouet's secret patterns (no gitleaks config found)".into(),
        }
    }
}

/// Reads and digests one transcript; `None` when it is outside `project`
/// or cannot be read.
pub(crate) fn digest(
    found: &Found<'_>,
    project: Option<&Path>,
    redactor: &Redactor,
) -> Option<Digest> {
    let transcript = read(found, project, true)?;
    let home = jouet_audit::home().map(|h| h.to_string_lossy().into_owned());
    let redact = |text: &str| redactor.redact(text);
    Some(digest::digest(
        found.reader.harness(),
        &transcript,
        &found.source.path.to_string_lossy(),
        &Options {
            home: home.as_deref(),
            redact: &redact,
        },
    ))
}

/// Digests the transcript a hook names, for the stop hook's threshold.
pub(crate) fn digest_path(harness: &str, path: &Path, redactor: &Redactor) -> Option<Digest> {
    let readers = Readers::from_env(Some(harness)).ok()?;
    let reader = readers
        .for_path(path)
        .or_else(|| readers.all().first().copied())?;
    let source = Source::from_path(path.to_path_buf()).ok()?;
    digest(&Found { reader, source }, None, redactor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_come_from_file_names() {
        assert_eq!(
            id_of(Path::new("/p/-work-app/d1d1-0000.jsonl")),
            "d1d1-0000"
        );
        assert_eq!(
            id_of(Path::new(
                "/s/rollout-2026-10-30T10-00-00-01a00000-0000-7000-8000-00000000d1d1.jsonl.zst"
            )),
            "01a00000-0000-7000-8000-00000000d1d1"
        );
    }
}
