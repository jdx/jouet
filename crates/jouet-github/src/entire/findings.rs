//! Review findings on an Entire trail, as `entire trail finding --json`
//! lists them, and the trail itself as `entire trail show --json` does.
//!
//! A finding's lifecycle (`open`, `resolved`, `dismissed`) is Entire's own
//! and separate from any review thread on the forge: resolving one never
//! resolves the other.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Characters of a body a finding's title keeps when Entire gives none.
const TITLE_WIDTH: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// Entire's ID, a ULID such as `01M65ECBREMZRY2QJEYDHEDYBA`.
    pub id: String,
    /// `open`, `resolved` or `dismissed`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Found on an earlier version of the code than the trail's current one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// Entire's title, or the body's first sentence when it has none.
    pub title: String,
    pub body: String,
    /// Who raised it, such as `agent:trail-review`.
    pub author: String,
    pub created_at: String,
    /// Why it was resolved or dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
}

impl Finding {
    pub fn is_open(&self) -> bool {
        self.status == "open"
    }

    /// `src/x.rs:12`, `src/x.rs:12-14`, `src/x.rs`, or `trail` for a
    /// finding on no file.
    pub fn location(&self) -> String {
        let Some(path) = &self.path else {
            return "trail".into();
        };
        match (self.line, self.end_line) {
            (Some(start), Some(end)) if end > start => format!("{path}:{start}-{end}"),
            (Some(line), _) => format!("{path}:{line}"),
            _ => path.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFinding {
    id: String,
    status: String,
    title: Option<String>,
    #[serde(default)]
    body: String,
    severity: Option<String>,
    status_reason: Option<String>,
    stale_outcome: Option<String>,
    #[serde(default)]
    actor_id: String,
    #[serde(default)]
    created_at: String,
    location: Option<RawLocation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLocation {
    file_path: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
}

/// A page of `entire trail finding --json`: its findings and whether more
/// pages follow.
pub(crate) fn parse(doc: &Value) -> Result<(Vec<Finding>, bool), String> {
    let raw: Vec<RawFinding> = Vec::deserialize(&doc["findings"])
        .map_err(|e| format!("entire sent findings jouet could not read: {e}"))?;
    let findings = raw
        .into_iter()
        .map(|r| {
            let location = r.location.unwrap_or(RawLocation {
                file_path: None,
                start_line: None,
                end_line: None,
            });
            Finding {
                title: r
                    .title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| first_sentence(&r.body)),
                id: r.id,
                status: r.status,
                severity: r.severity,
                stale: r.stale_outcome.as_deref() == Some("stale"),
                path: location.file_path.filter(|p| !p.is_empty()),
                line: location.start_line,
                end_line: location.end_line,
                body: r.body,
                author: r.actor_id,
                created_at: r.created_at,
                status_reason: r.status_reason,
            }
        })
        .collect();
    Ok((findings, doc["has_more"].as_bool().unwrap_or(false)))
}

/// The first sentence of `body`, cut at a word to `TITLE_WIDTH` characters.
fn first_sentence(body: &str) -> String {
    let line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let sentence = match line.find(". ") {
        Some(i) => &line[..=i],
        None => line,
    }
    .trim();
    if sentence.chars().count() <= TITLE_WIDTH {
        return sentence.to_string();
    }
    let head: String = sentence.chars().take(TITLE_WIDTH).collect();
    let head = match head.rfind(' ') {
        Some(i) if i >= TITLE_WIDTH / 2 => &head[..i],
        _ => &head,
    };
    format!("{}...", head.trim_end())
}

/// Characters of a ULID a handle shows, unless two findings share them.
const HANDLE: usize = 10;
/// The prefix that marks an Entire finding's handle, so it is never taken
/// for a forge thread's number.
pub const PREFIX: &str = "entire:";

/// The handles for findings: `entire:` and the first ten characters of each
/// ID, more where two would otherwise be the same.
pub fn handles(ids: &[&str]) -> Vec<String> {
    ids.iter()
        .map(|id| {
            let mut n = HANDLE.min(id.len());
            while n < id.len()
                && ids
                    .iter()
                    .any(|other| other != id && other.get(..n) == id.get(..n))
            {
                n += 1;
            }
            format!("{PREFIX}{}", &id[..n])
        })
        .collect()
}

/// A finding in a PR's status: enough to name it and say where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brief {
    pub id: String,
    /// What `jouet pr threads` takes for it, such as `entire:01M4C997EA`.
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub title: String,
    #[serde(skip)]
    pub location: String,
}

impl Brief {
    /// Briefs for findings, each with its handle.
    pub fn of(findings: &[Finding]) -> Vec<Self> {
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        findings
            .iter()
            .zip(handles(&ids))
            .map(|(f, handle)| Self {
                id: f.id.clone(),
                handle,
                severity: f.severity.clone(),
                path: f.path.clone(),
                line: f.line,
                title: f.title.clone(),
                location: f.location(),
            })
            .collect()
    }
}

/// A trail, as much of `entire trail show --json` as jouet reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trail {
    pub number: u64,
    pub url: String,
    /// The branch, or for a merged trail the one it had.
    pub branch: String,
    /// `draft`, `open`, `merged` or `closed`.
    pub status: String,
}

impl Trail {
    pub(crate) fn parse(doc: &Value) -> Option<Self> {
        let s = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        Some(Self {
            number: doc["number"].as_u64()?,
            url: s(&doc["url"]).unwrap_or_default(),
            branch: s(&doc["branch"])
                .or_else(|| s(&doc["original_branch"]))
                .unwrap_or_default(),
            status: s(&doc["status"]).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISE_424: &str = include_str!("fixtures/finding-mise-424.json");
    const MISE_1159_ANY: &str = include_str!("fixtures/finding-mise-1159-any.json");
    const SHOW_JOUET_87: &str = include_str!("fixtures/show-jouet-87.json");

    #[test]
    fn open_findings_have_a_location_and_a_title_from_the_body() {
        let doc: Value = serde_json::from_str(MISE_424).unwrap();
        let (findings, more) = parse(&doc).unwrap();
        assert!(!more);
        assert_eq!(findings.len(), 2);
        let f = &findings[0];
        assert_eq!(f.id, "01M4C997EAAK13R52CVH7GYY7M");
        assert!(f.is_open() && !f.stale);
        assert_eq!(f.severity.as_deref(), Some("medium"));
        assert_eq!(
            f.location(),
            "e2e/tasks/test_task_monorepo_deps_tool_install:44"
        );
        assert!(f.title.len() <= TITLE_WIDTH + 3, "{}", f.title);
        assert_eq!(f.author, "agent:trail-review");
    }

    #[test]
    fn resolved_and_stale_findings_say_so() {
        let doc: Value = serde_json::from_str(MISE_1159_ANY).unwrap();
        let (findings, _) = parse(&doc).unwrap();
        let resolved = findings.iter().find(|f| f.status == "resolved").unwrap();
        assert!(resolved.stale);
        assert!(
            resolved
                .status_reason
                .as_deref()
                .unwrap()
                .starts_with("Fixed in 58b789e5d2")
        );
    }

    #[test]
    fn a_trail_keeps_its_branch_once_merged() {
        let doc: Value = serde_json::from_str(SHOW_JOUET_87).unwrap();
        let t = Trail::parse(&doc).unwrap();
        assert_eq!(t.number, 87);
        assert_eq!(t.branch, "fix/cmd-through-wrappers");
        assert_eq!(t.status, "open");
        let merged = serde_json::json!({"number": 86, "branch": "", "original_branch": "fix/x", "status": "merged"});
        assert_eq!(Trail::parse(&merged).unwrap().branch, "fix/x");
    }

    #[test]
    fn handles_grow_only_where_two_would_clash() {
        let ids = [
            "01M4C997EAAK13R52CVH7GYY7M",
            "01M4C997DZH5KQRP31NW93NQHR",
            "01M4C997EAZZZZZZZZZZZZZZZZ",
        ];
        assert_eq!(
            handles(&ids),
            [
                "entire:01M4C997EAA",
                "entire:01M4C997DZ",
                "entire:01M4C997EAZ"
            ]
        );
    }

    #[test]
    fn titles_stop_at_the_first_sentence() {
        assert_eq!(first_sentence("One thing. Then more."), "One thing.");
        assert_eq!(first_sentence("\nno stop"), "no stop");
        let long = "word ".repeat(40);
        assert!(first_sentence(&long).ends_with("..."));
    }
}
