//! `jouet sessions`: which agent sessions are working where on this machine.
//! The transcripts are read by jouet-audit; this decides what is live and
//! what to warn about, and redacts what it shows as the ledger does.

use super::Output;
use super::audit::parse_since;
use crate::cli::SessionsArgs;
use crate::entire::{self, Entire};
use crate::ledger::redact_id;
use jouet_audit::redact;
use jouet_audit::sessions::{self, Session};
use jouet_audit::{ClaudeCode, Codex};
use jouet_output::exit::Exit;
use jouet_output::{paths, time};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A session whose transcript was written this recently is live.
const LIVE_SECS: i64 = 15 * 60;
const DEFAULT_SINCE: &str = "24h";

pub(crate) fn run(args: &SessionsArgs, out: &Output) -> Exit {
    let now = time::now();
    let since = match parse_since(args.since.as_deref().unwrap_or(DEFAULT_SINCE), now) {
        Ok(secs) => secs,
        Err(e) => return out.error(Exit::Usage, &format!("--since: {e}")),
    };
    let mut found = sessions::recent(
        ClaudeCode::from_env().as_ref(),
        Codex::from_env().as_ref(),
        since.saturating_mul(1000),
    );
    let mut notes = Vec::new();
    let tracked = match std::env::current_dir().ok().and_then(|d| Entire::here(&d)) {
        Some(entire) => entire.sessions().unwrap_or_else(|e| {
            notes.push(e);
            Vec::new()
        }),
        None => Vec::new(),
    };
    let only_entire = only_in_entire(&tracked, &found, since);
    found.extend(only_entire.iter().cloned());
    let mut report = report(found, now, current_session().as_deref(), args.all);
    attach_entire(&mut report, &tracked, &only_entire);
    report.notes = notes;
    out.result(&report, || text(&report));
    Exit::Ok
}

/// The sessions Entire tracks in this repository that no transcript jouet
/// read accounts for, active since `since`, as sessions to list: an agent
/// jouet does not read, or a transcript kept elsewhere. Entire records the
/// directory the agent started in, which can be the main checkout for a
/// session working in a worktree, so a session jouet found keeps its own.
fn only_in_entire(tracked: &[entire::Session], found: &[Session], since: i64) -> Vec<Session> {
    let now = time::now();
    tracked
        .iter()
        .filter(|t| !found.iter().any(|f| f.id == t.session_id))
        .filter_map(|t| {
            let last = time::parse_time(t.last_active.as_deref()?, now).ok()?;
            (last >= since).then(|| Session {
                harness: harness_of(&t.agent),
                id: t.session_id.clone(),
                cwd: t.worktree_path.clone(),
                branch: None,
                modified_ms: last.saturating_mul(1000),
                transcript: PathBuf::new(),
            })
        })
        .collect()
}

/// The harness name jouet uses for an agent as Entire names it.
fn harness_of(agent: &str) -> &'static str {
    match agent.to_ascii_lowercase().as_str() {
        "claude code" => "claude",
        "codex" => "codex",
        "cursor" => "cursor",
        "opencode" => "opencode",
        "gemini cli" => "gemini",
        "pi" => "pi",
        _ => "agent",
    }
}

/// Adds what Entire records to the rows of the sessions it tracks.
fn attach_entire(report: &mut Report, tracked: &[entire::Session], only: &[Session]) {
    for row in &mut report.sessions {
        let Some(t) = tracked.iter().find(|t| redact_id(&t.session_id) == row.id) else {
            continue;
        };
        row.entire = Some(EntireRow {
            status: t.status.clone(),
            checkpoints: t.checkpoints,
            only: only.iter().any(|s| s.id == t.session_id),
        });
    }
}

/// The session jouet runs in, so its row can say so.
pub(crate) fn current_session() -> Option<String> {
    [
        "JOUET_SESSION_ID",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
    ]
    .iter()
    .filter_map(|name| std::env::var(name).ok())
    .find(|v| !v.trim().is_empty())
}

#[derive(Serialize)]
struct Report {
    sessions: Vec<Row>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<Warning>,
    /// Idle sessions in the window that are not listed (without --all).
    #[serde(skip_serializing_if = "is_zero")]
    idle_not_listed: usize,
    /// Why Entire's sessions could not be read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

/// What Entire records of a session, where this repository has Entire.
#[derive(Serialize)]
struct EntireRow {
    /// `active`, `idle` or `ended`.
    #[serde(skip_serializing_if = "String::is_empty")]
    status: String,
    #[serde(skip_serializing_if = "is_zero_u64")]
    checkpoints: u64,
    /// Only Entire knows the session: jouet read no transcript of it.
    #[serde(skip_serializing_if = "is_false")]
    only: bool,
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

#[derive(Serialize)]
struct Row {
    harness: &'static str,
    id: String,
    /// The directory the session last worked in.
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// The work tree that directory is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    /// The branch checked out there now, else the one the transcript
    /// recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    /// When the transcript was last written, UTC.
    last_active: String,
    idle_secs: i64,
    #[serde(skip_serializing_if = "is_false")]
    live: bool,
    /// The session jouet is running in.
    #[serde(skip_serializing_if = "is_false")]
    current: bool,
    /// The directory it worked in no longer exists.
    #[serde(skip_serializing_if = "is_false")]
    gone: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    entire: Option<EntireRow>,
    /// For grouping only: the work tree, unredacted.
    #[serde(skip)]
    root: Option<PathBuf>,
}

#[derive(Serialize)]
struct Warning {
    /// `shared_worktree` or `worktree_gone`.
    code: &'static str,
    message: String,
    /// The sessions it is about: `harness:id`.
    sessions: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn report(found: Vec<Session>, now: i64, current: Option<&str>, all: bool) -> Report {
    let home = jouet_audit::home();
    let mut rows: Vec<Row> = found
        .into_iter()
        .map(|s| row(s, now, current, home.as_deref()))
        .collect();
    let mut warnings = Vec::new();
    let mut by_tree: BTreeMap<&Path, Vec<&Row>> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.live) {
        if let Some(root) = &r.root {
            by_tree.entry(root).or_default().push(r);
        }
    }
    for (root, live) in by_tree.into_iter().filter(|(_, l)| l.len() > 1) {
        let names: Vec<String> = live.iter().map(|r| name(r)).collect();
        warnings.push(Warning {
            code: "shared_worktree",
            message: format!(
                "{} live sessions share the work tree {} ({}): each can commit the other's uncommitted work; give one its own worktree",
                live.len(),
                show_path(root, home.as_deref()),
                names.join(", ")
            ),
            sessions: live.iter().map(|r| key(r)).collect(),
        });
    }
    // An idle session's worktree is usually gone because the work ended and
    // it was cleaned up; a live one's going is the surprise.
    for r in rows.iter().filter(|r| r.gone && r.live) {
        warnings.push(Warning {
            code: "worktree_gone",
            message: format!(
                "{} last worked in {}, which no longer exists",
                name(r),
                r.cwd.as_deref().unwrap_or("?")
            ),
            sessions: vec![key(r)],
        });
    }
    let warned: Vec<String> = warnings.iter().flat_map(|w| w.sessions.clone()).collect();
    let before = rows.len();
    if !all {
        rows.retain(|r| r.live || warned.contains(&key(r)));
    }
    Report {
        idle_not_listed: before - rows.len(),
        sessions: rows,
        warnings,
        notes: Vec::new(),
    }
}

fn row(s: Session, now: i64, current: Option<&str>, home: Option<&Path>) -> Row {
    let last = s.modified_ms / 1000;
    let idle = (now - last).max(0);
    let gone = s.cwd.as_deref().is_some_and(|c| !Path::new(c).exists());
    // The lookup walks up from the directory, so a removed worktree inside
    // another checkout would resolve to that checkout.
    let root = if gone {
        None
    } else {
        s.cwd
            .as_deref()
            .and_then(|c| paths::work_tree(Path::new(c)))
    };
    let branch = root.as_deref().and_then(sessions::head_branch).or(s.branch);
    Row {
        harness: s.harness,
        current: current == Some(s.id.as_str()),
        id: redact_id(&s.id),
        cwd: s.cwd.as_deref().map(|c| show_path(Path::new(c), home)),
        worktree: root.as_deref().map(|r| show_path(r, home)),
        branch: branch.map(|b| b.split('/').map(redact).collect::<Vec<_>>().join("/")),
        last_active: time::format_utc(last),
        idle_secs: idle,
        live: idle <= LIVE_SECS,
        gone,
        entire: None,
        root,
    }
}

/// A path as reports show one: the home directory as `~`, and anything
/// that looks like a credential redacted, as `jouet audit` redacts its
/// examples.
fn show_path(path: &Path, home: Option<&Path>) -> String {
    let (prefix, rest) = match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => ("~/", rest),
        None => ("", path),
    };
    // Part by part: a run of path separators and digits reads as a key.
    let text = rest.to_string_lossy();
    let parts: Vec<String> = text.split('/').map(redact).collect();
    let joined = format!("{prefix}{}", parts.join("/"));
    if joined == "~/" { "~".into() } else { joined }
}

fn key(r: &Row) -> String {
    format!("{}:{}", r.harness, r.id)
}

/// A session as text names it: its harness and the start of its ID.
fn name(r: &Row) -> String {
    format!("{} {}", r.harness, short(&r.id))
}

fn checkpoints(n: u64) -> String {
    format!("{n} checkpoint{}", if n == 1 { "" } else { "s" })
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn text(report: &Report) -> String {
    let mut s = String::new();
    if report.sessions.is_empty() {
        s.push_str("no live agent sessions on this machine\n");
    }
    for r in &report.sessions {
        let state = if r.live { "live" } else { "idle" };
        let mark = if r.current { "*" } else { " " };
        let place = r
            .worktree
            .as_deref()
            .or(r.cwd.as_deref())
            .unwrap_or("(no directory recorded)");
        let mut line = format!(
            "{mark}{state}  {:6}  {:8}  {:>8}  {place}",
            r.harness,
            short(&r.id),
            format!("{} ago", time::age(r.idle_secs)),
        );
        if r.gone {
            line.push_str("  (gone)");
        } else if let Some(branch) = &r.branch {
            let _ = write!(line, "  [{branch}]");
        }
        match &r.entire {
            Some(e) if e.only => {
                let _ = write!(line, "  (only in Entire, {})", checkpoints(e.checkpoints));
            }
            Some(e) if e.checkpoints > 0 => {
                let _ = write!(line, "  (Entire: {})", checkpoints(e.checkpoints));
            }
            _ => {}
        }
        let _ = writeln!(s, "{}", line.trim_end());
    }
    if report.sessions.iter().any(|r| r.current) {
        s.push_str("(* this session)\n");
    }
    if !report.warnings.is_empty() {
        s.push('\n');
        for w in &report.warnings {
            let _ = writeln!(s, "warning: {}", w.message);
        }
    }
    for note in &report.notes {
        let _ = writeln!(s, "note: {note}");
    }
    if report.idle_not_listed > 0 {
        let _ = writeln!(
            s,
            "\n... {} idle session{} not listed (--all lists them)",
            report.idle_not_listed,
            if report.idle_not_listed == 1 { "" } else { "s" }
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, cwd: &Path, idle: i64, now: i64) -> Session {
        Session {
            harness: "claude",
            id: id.into(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            branch: Some("from/transcript".into()),
            modified_ms: (now - idle) * 1000,
            transcript: PathBuf::new(),
        }
    }

    #[test]
    fn warns_about_a_shared_work_tree_and_a_gone_one() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join(".git")).unwrap();
        std::fs::write(tree.join(".git/HEAD"), "ref: refs/heads/feat/a\n").unwrap();
        std::fs::create_dir_all(tree.join("src")).unwrap();
        let now = 2_000_000_000;
        let found = vec![
            session("aaaaaaaa-1", &tree, 60, now),
            session("bbbbbbbb-2", &tree.join("src"), 120, now),
            session("cccccccc-3", &dir.path().join("vanished"), 300, now),
            session("dddddddd-4", &tree, 3 * 3600, now),
            session("eeeeeeee-5", &dir.path().join("cleaned-up"), 7200, now),
        ];
        let r = report(found, now, Some("aaaaaaaa-1"), false);
        let codes: Vec<&str> = r.warnings.iter().map(|w| w.code).collect();
        assert_eq!(codes, ["shared_worktree", "worktree_gone"]);
        assert_eq!(
            r.warnings[0].sessions,
            ["claude:aaaaaaaa-1", "claude:bbbbbbbb-2"]
        );
        // Idle sessions are not listed, even one whose tree is gone.
        let ids: Vec<&str> = r.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["aaaaaaaa-1", "bbbbbbbb-2", "cccccccc-3"]);
        assert_eq!(r.idle_not_listed, 2);
        assert!(r.sessions[0].current && r.sessions[0].live);
        // The branch is the one checked out now, not the transcript's.
        assert_eq!(r.sessions[1].branch.as_deref(), Some("feat/a"));
        assert_eq!(r.sessions[2].branch.as_deref(), Some("from/transcript"));
        assert!(r.sessions[2].gone && r.sessions[2].live);

        let all = report(
            vec![session("dddddddd-4", &tree, 3 * 3600, now)],
            now,
            None,
            true,
        );
        assert_eq!(all.sessions.len(), 1);
        assert!(all.warnings.is_empty());
    }

    #[test]
    fn a_removed_worktree_inside_a_checkout_is_not_that_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join(".git")).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let removed = main.join(".claude/worktrees/feat-x");
        let now = 2_000_000_000;
        let r = report(
            vec![
                session("aaaaaaaa-1", &main, 60, now),
                session("bbbbbbbb-2", &removed, 120, now),
            ],
            now,
            None,
            false,
        );
        let codes: Vec<&str> = r.warnings.iter().map(|w| w.code).collect();
        assert_eq!(codes, ["worktree_gone"]);
        let gone = &r.sessions[1];
        assert!(gone.gone);
        assert_eq!(gone.worktree, None);
        assert_eq!(gone.branch.as_deref(), Some("from/transcript"));
    }

    #[test]
    fn paths_show_home_as_a_tilde_and_lose_secrets() {
        let home = Path::new("/home/me");
        assert_eq!(
            show_path(Path::new("/home/me/src/x"), Some(home)),
            "~/src/x"
        );
        assert_eq!(show_path(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(
            show_path(
                Path::new("/tmp/ghp_abcdefghijklmnopqrstuvwxyz/x"),
                Some(home)
            ),
            "/tmp/<redacted>/x"
        );
    }
}
