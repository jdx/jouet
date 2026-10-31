//! Memory upkeep: noticing when an agent's saved memories have gone stale.
//!
//! Agents save memories, and many of them work around a bug: "until #886
//! ships, run cargo through `mise x`". The bug gets fixed and later sessions
//! keep applying the workaround. Wind reads the memories (Claude Code's
//! per-project notes, `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md` and
//! Codex's consolidated memories), pulls out the claims each one depends on,
//! and checks them without a model:
//!
//! - a PR or issue the memory says it waits on, now merged or closed;
//! - a merged PR the memory says is not released yet, now in a release;
//! - a `jouet` command or flag it names that jouet no longer has;
//! - a built-in rule it says to override, which makes it a jouet bug.
//!
//! What it finds becomes a memory note, shown at a session start in the
//! memory's project under the ledger notes' limits. jouet never edits a
//! memory: the agent there asks the user. The stop hook runs the check in
//! the background at most once a day, since it asks GitHub.

use super::{MAX_NEW_PER_DAY, Paths, State, Status};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The check runs at most this often.
const EVERY: u64 = 24 * 60 * 60;
/// A lookup that can still change (an open PR, a merge not yet released)
/// is asked again after this long.
const RECHECK: u64 = 20 * 60 * 60;
/// References asked about in one GraphQL query, at most.
const PER_QUERY: usize = 80;

/// Where a memory comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Origin {
    /// A note in `~/.claude/projects/<project>/memory/`.
    Claude,
    /// `~/.claude/CLAUDE.md`.
    ClaudeGlobal,
    /// `~/.codex/AGENTS.md`.
    CodexGlobal,
    /// `~/.codex/memories/MEMORY.md` or `memory_summary.md`.
    Codex,
}

/// One memory, as read.
#[derive(Debug, Clone)]
pub(crate) struct Memory {
    pub origin: Origin,
    pub path: PathBuf,
    /// Its frontmatter `name`, or the file's name.
    pub name: String,
    pub description: String,
    /// The Claude Code project folder it belongs to, such as
    /// `-home-me-src-app`; `None` for a memory every session reads.
    pub project: Option<String>,
    /// Its text, without the frontmatter.
    pub body: String,
}

impl Memory {
    /// Its path with the home directory as `~`.
    pub(crate) fn display(&self) -> String {
        tilde(&self.path)
    }
}

/// The path with the home directory as `~`.
pub(crate) fn tilde(path: &Path) -> String {
    match jouet_audit::home().and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Where the harnesses keep their memories.
#[derive(Debug, Clone)]
pub(crate) struct Roots {
    /// `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
    pub claude: Option<PathBuf>,
    /// `$CODEX_HOME`, or `~/.codex`.
    pub codex: Option<PathBuf>,
}

impl Roots {
    pub(crate) fn from_env() -> Self {
        let home = std::env::home_dir();
        let dir = |var: &str, name: &str| {
            jouet_output::paths::env_dir(var).or_else(|| home.as_ref().map(|h| h.join(name)))
        };
        Self {
            claude: dir("CLAUDE_CONFIG_DIR", ".claude"),
            codex: dir("CODEX_HOME", ".codex"),
        }
    }

    /// Whether there is any memory to read, without reading one.
    pub(crate) fn any(&self) -> bool {
        let claude = self.claude.as_ref().is_some_and(|c| {
            c.join("CLAUDE.md").is_file()
                || std::fs::read_dir(c.join("projects"))
                    .is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().join("memory").is_dir())))
        });
        let codex = self.codex.as_ref().is_some_and(|c| {
            c.join("AGENTS.md").is_file() || c.join("memories").join("MEMORY.md").is_file()
        });
        claude || codex
    }
}

/// Every memory under `roots`, in a stable order. Codex's
/// `memories_1.sqlite` is left out: it holds per-session extractions that
/// Codex consolidates into `memories/MEMORY.md` itself.
pub(crate) fn discover(roots: &Roots) -> Vec<Memory> {
    let mut out = Vec::new();
    if let Some(claude) = &roots.claude {
        let mut projects: Vec<PathBuf> = std::fs::read_dir(claude.join("projects"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("memory").is_dir())
            .collect();
        projects.sort();
        for project in projects {
            // The folder is only ever joined as a path: its name starts
            // with `-`, which a command would read as an option.
            let folder = project
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            let mut files: Vec<PathBuf> = std::fs::read_dir(project.join("memory"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "md"))
                .filter(|p| p.file_name().is_some_and(|n| n != "MEMORY.md"))
                .collect();
            files.sort();
            for path in files {
                if let Some(m) = read(Origin::Claude, &path, folder.clone()) {
                    out.push(m);
                }
            }
        }
        out.extend(read(Origin::ClaudeGlobal, &claude.join("CLAUDE.md"), None));
    }
    if let Some(codex) = &roots.codex {
        out.extend(read(Origin::CodexGlobal, &codex.join("AGENTS.md"), None));
        for name in ["MEMORY.md", "memory_summary.md"] {
            out.extend(read(
                Origin::Codex,
                &codex.join("memories").join(name),
                None,
            ));
        }
    }
    out
}

fn read(origin: Origin, path: &Path, project: Option<String>) -> Option<Memory> {
    let text = std::fs::read_to_string(path).ok()?;
    let (front, body) = frontmatter(&text);
    let field = |key: &str| {
        front.lines().find_map(|l| {
            let value = l.strip_prefix(key)?.strip_prefix(':')?.trim();
            Some(value.trim_matches('"').trim_matches('\'').to_string())
        })
    };
    let file = path.file_name()?.to_string_lossy().into_owned();
    let name = match origin {
        Origin::Claude => field("name").unwrap_or_else(|| file.trim_end_matches(".md").into()),
        _ => tilde(path),
    };
    let description = field("description").unwrap_or_else(|| {
        body.lines()
            .map(|l| l.trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .to_string()
    });
    Some(Memory {
        origin,
        path: path.to_path_buf(),
        name,
        description,
        project,
        body: body.to_string(),
    })
}

/// A memory's YAML frontmatter and the text after it.
fn frontmatter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    match rest.find("\n---") {
        Some(end) => {
            let body = &rest[end + 4..];
            (&rest[..end], body.strip_prefix('\n').unwrap_or(body))
        }
        None => ("", text),
    }
}

/// A directory as Claude Code names its project folder: every character
/// that is not an ASCII letter or digit becomes `-`.
pub(crate) fn folder_of(dir: &Path) -> String {
    dir.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The main work tree of the one containing `cwd`: for a linked worktree,
/// the checkout it was added from, which is where Claude Code keeps the
/// project's memories. Read from the filesystem, as a hook cannot afford
/// to start git.
fn main_work_tree(cwd: &Path) -> Option<PathBuf> {
    let root = jouet_output::paths::work_tree(cwd)?;
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(root);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = root.join(text.trim().strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = gitdir.join(common.trim()).canonicalize().ok()?;
    Some(common.parent()?.to_path_buf())
}

/// Whether a session running in `cwd` reads the memories of `project`.
pub(crate) fn in_project(project: &str, cwd: &Path) -> bool {
    folder_of(cwd) == project
        || jouet_output::paths::work_tree(cwd).is_some_and(|w| folder_of(&w) == project)
        || main_work_tree(cwd).is_some_and(|w| folder_of(&w) == project)
}

/// The directory a project folder names, found by walking the filesystem
/// and keeping only directories whose name encodes to the next part:
/// decoding the name alone cannot tell `a-b` from `a/b` or `a.b`.
pub(crate) fn project_dir(folder: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, rest: &str, depth: usize) -> Option<PathBuf> {
        if rest.is_empty() {
            return Some(dir.to_path_buf());
        }
        if depth > 24 {
            return None;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        // Longer names first, so `a-b` is tried before `a` then `b`.
        entries.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
        entries.into_iter().find_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            let part = format!("-{}", folder_of(Path::new(&name)));
            let next = rest.strip_prefix(part.as_str())?;
            if !next.is_empty() && !next.starts_with('-') {
                return None;
            }
            walk(&p, next, depth + 1)
        })
    }
    if !cfg!(unix) {
        return None;
    }
    walk(Path::new("/"), folder, 0)
}

/// The GitHub repository a checkout's `origin` points to, as `owner/name`,
/// read from its git config.
pub(crate) fn origin_repo(dir: &Path) -> Option<String> {
    let config = std::fs::read_to_string(dir.join(".git").join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
            continue;
        }
        if in_origin && let Some(url) = line.strip_prefix("url") {
            let url = url.trim_start().strip_prefix('=')?.trim();
            let path = url
                .strip_prefix("git@github.com:")
                .or_else(|| url.strip_prefix("https://github.com/"))
                .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
            let path = path.trim_end_matches('/').trim_end_matches(".git");
            return valid_repo(path).then(|| path.to_string());
        }
    }
    None
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// What a memory says about a PR or issue it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Said {
    /// It waits on it: "until #12 merges", "still open", "when to drop".
    Waiting,
    /// It records it as done: "fixed in #12", "#12 (merged …)".
    Done,
}

/// A PR or issue a memory names, and what it says of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reference {
    /// `owner/name#number`.
    pub key: String,
    pub said: Option<Said>,
    /// The memory says it is merged but not released yet.
    pub awaits_release: bool,
}

/// What a memory depends on that can be checked.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Claims {
    pub references: Vec<Reference>,
    /// `jouet` command lines it names, as the words after `jouet`.
    pub commands: Vec<Vec<String>>,
    /// Built-in rules it names in a sentence about getting past them, with
    /// that sentence.
    pub overrides: Vec<(String, String)>,
}

static URL_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:https?://)?github\.com/([A-Za-z0-9][\w.-]*/[\w.-]+)/(?:pull|issues)/(\d+)")
        .expect("valid regex")
});
static FULL_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^\w/.-])([A-Za-z0-9][\w.-]*/[\w.-]+)#(\d+)\b").expect("valid regex")
});
static SHORT_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^\w/.-])([A-Za-z0-9][\w.-]*)#(\d+)\b").expect("valid regex")
});
static BARE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^\w/&#\]-])#(\d+)\b").expect("valid regex"));
/// Waiting, said anywhere in a sentence: "pending", "waits on", "until it
/// merges", "when to drop this".
static WAITING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(?:not\ (?:yet\ )?merged|isn'?t\ merged|unmerged|still\ open|pending|waits?\ (?:on|for)
          |waiting\ (?:on|for)|blocked\ (?:on|by)|when\ to\ drop|drop\ this|remove\ this|delete\ this
          |(?:until|once)\b[^.;:]{0,60}\b(?:merges?|merged|lands?|landed|ships?|shipped|released?|fixed|closed|is\ out))\b",
    )
    .expect("valid regex")
});
/// Waiting, said right after the reference: "#12 is open", "#12, open",
/// "#12 (the fix) is still open". Elsewhere in a sentence, "while a branch
/// is open" is about something else.
static OPEN_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[\s)\]]*(?:\([^)]{0,40}\)[\s,]*)?(?:,\s*|\(\s*)?(?:(?:is|are|was|were|still|stays?)\s+(?:still\s+)?)?open\b",
    )
    .expect("valid regex")
});
/// Waiting, said right before it: "open PR #12".
static OPEN_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bopen\s+(?:PRs?|issues?|pull\s+requests?)?[\s:]*\[?$").expect("valid regex")
});
static DONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:merged|closed|fixed\ (?:in|by)|landed|shipped|resolved|released|first\ in|(?:on|in)\ (?:main|master))\b")
        .expect("valid regex")
});
static NEGATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\bnot|n't|\bnever|\bun)\s*(?:yet\s+)?(?:merged|released|landed|shipped|fixed|closed)")
        .expect("valid regex")
});
/// The memory waits for a release, not a merge.
static RELEASE_WAIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(?:not\ (?:yet\ )?in\ a\ release|isn'?t\ in\ a\ release|in\ no\ release|not\ (?:yet\ )?released
          |isn'?t\ released|predates\ it|next\ release|none\ in\ (?:the\ )?\S+\ release
          |waits?\ for\ (?:the\ |a\ )?(?:first\ |next\ )?release
          |(?:until|once)\b[^.;:]{0,60}\b(?:releases?|released|ships?|shipped|is\ out))\b",
    )
    .expect("valid regex")
});
/// "Unreleased", right beside the reference: elsewhere in a sentence it is
/// often about something else.
static UNRELEASED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bunreleased\b").expect("valid regex"));
/// A sentence saying a command is not there, or not yet.
static ABSENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:no|not|never|skipped|skips|without|removed|dropped|renamed|planned|proposed|would|instead\ of)\b|n't\b")
        .expect("valid regex")
});
static OVERRIDE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:overrid\w*|bypass\w*|work\s?around|disable[sd]?|turn\w* off|wrongly|false positive)")
        .expect("valid regex")
});

/// The memory's text in sentences, grouped into blocks (a paragraph or a
/// list item), with code fences left out.
fn blocks(body: &str) -> Vec<Vec<String>> {
    let mut joined: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut fenced = false;
    let flush = |current: &mut String, joined: &mut Vec<String>| {
        if !current.trim().is_empty() {
            joined.push(std::mem::take(current));
        }
        current.clear();
    };
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            flush(&mut current, &mut joined);
            continue;
        }
        if fenced {
            continue;
        }
        let item = trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with('#')
            || trimmed
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if trimmed.is_empty() || item {
            flush(&mut current, &mut joined);
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(trimmed);
    }
    flush(&mut current, &mut joined);
    joined.iter().map(|b| sentences(b)).collect()
}

fn sentences(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = block.as_bytes();
    for (i, c) in block.char_indices() {
        if matches!(c, '.' | '!' | '?' | ';')
            && bytes.get(i + 1).is_some_and(u8::is_ascii_whitespace)
        {
            out.push(block[start..=i].trim().to_string());
            start = i + 1;
        }
    }
    if !block[start..].trim().is_empty() {
        out.push(block[start..].trim().to_string());
    }
    out
}

/// What a sentence says of the reference at `at..end`, given every
/// reference in it (`refs`): what is said right beside it wins; then a
/// cue elsewhere in the sentence, when this is the reference nearest it. A
/// sentence that says both is left undecided.
fn said(sentence: &str, at: usize, end: usize, refs: &[(usize, usize)]) -> Option<Said> {
    let from = floor(sentence, at.saturating_sub(24));
    let to = floor(sentence, (end + 24).min(sentence.len()));
    let window = &sentence[from..to];
    let after = &sentence[end..floor(sentence, (end + 48).min(sentence.len()))];
    let before = &sentence[floor(sentence, at.saturating_sub(24))..at];
    // "#12 (merged …)" or "#12 is still open", right after it, decides.
    if done(&after[..floor(after, after.len().min(24))]) {
        return Some(Said::Done);
    }
    if OPEN_AFTER.is_match(after) {
        return Some(Said::Waiting);
    }
    let waiting = WAITING.is_match(window) || OPEN_BEFORE.is_match(before);
    match (waiting, done(window)) {
        (true, false) => return Some(Said::Waiting),
        (false, true) => return Some(Said::Done),
        _ => {}
    }
    let distance = |pos: usize, r: &(usize, usize)| {
        if pos < r.0 {
            r.0 - pos
        } else {
            pos.saturating_sub(r.1)
        }
    };
    let nearest = |pos: usize| refs.iter().min_by_key(|r| distance(pos, r)).copied();
    let waiting = WAITING
        .find_iter(sentence)
        .any(|m| nearest(m.start()) == Some((at, end)));
    match (waiting, done(sentence)) {
        (true, false) => Some(Said::Waiting),
        (false, true) => Some(Said::Done),
        _ => None,
    }
}

/// Whether `text` says something is done: "merged", but not "not merged".
fn done(text: &str) -> bool {
    DONE.find_iter(text).any(|m| {
        let before = &text[floor(text, m.start().saturating_sub(10))..m.start()];
        !NEGATED.is_match(&format!("{before}{}", m.as_str()))
    })
}

fn floor(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The claims in a memory's text. `repo` is the project's own repository;
/// `rules` are jouet's built-in rule names. A bare `#12` belongs to the
/// repository named last before it in its paragraph, else to the one the
/// memory names, else, when it names none, to `repo`; when it names
/// several, it is left out.
pub(crate) fn claims(body: &str, repo: Option<&str>, rules: &[String]) -> Claims {
    let mut out = Claims::default();
    let named: BTreeSet<String> = FULL_REF
        .captures_iter(body)
        .chain(URL_REF.captures_iter(body))
        .map(|c| c[1].to_string())
        .collect();
    let fallback = match named.len() {
        0 => repo.map(str::to_string),
        1 => named.first().cloned(),
        _ => None,
    };
    // Owners named anywhere in the memory, for `name#12`.
    let owners: BTreeMap<String, String> = FULL_REF
        .captures_iter(body)
        .chain(URL_REF.captures_iter(body))
        .filter_map(|c| {
            let (owner, name) = c[1].split_once('/')?;
            Some((name.to_ascii_lowercase(), format!("{owner}/{name}")))
        })
        .chain(repo.and_then(|r| {
            let (_, name) = r.split_once('/')?;
            Some((name.to_ascii_lowercase(), r.to_string()))
        }))
        .collect();
    for block in blocks(body) {
        let mut last: Option<usize> = None;
        let mut last_repo: Option<String> = None;
        for sentence in &block {
            let mut found: Vec<(usize, usize, String)> = Vec::new();
            let push =
                |found: &mut Vec<(usize, usize, String)>, at: usize, end: usize, key: String| {
                    if !found.iter().any(|f| f.2 == key) {
                        found.push((at, end, key));
                    }
                };
            for c in URL_REF.captures_iter(sentence) {
                let m = c.get(0).expect("a match");
                push(
                    &mut found,
                    m.start(),
                    m.end(),
                    format!("{}#{}", &c[1], &c[2]),
                );
            }
            for c in FULL_REF.captures_iter(sentence) {
                let m = c.get(1).expect("a group");
                push(
                    &mut found,
                    m.start(),
                    c.get(0).expect("a match").end(),
                    format!("{}#{}", &c[1], &c[2]),
                );
            }
            for c in SHORT_REF.captures_iter(sentence) {
                let m = c.get(1).expect("a group");
                if let Some(full) = owners.get(&m.as_str().to_ascii_lowercase()) {
                    push(
                        &mut found,
                        m.start(),
                        c.get(0).expect("a match").end(),
                        format!("{full}#{}", &c[2]),
                    );
                }
            }
            for c in BARE_REF.captures_iter(sentence) {
                let m = c.get(0).expect("a match");
                // The repository named last before it, in this paragraph.
                let before = found
                    .iter()
                    .filter(|f| f.0 < m.start())
                    .max_by_key(|f| f.0)
                    .and_then(|f| f.2.split_once('#'))
                    .map(|(r, _)| r.to_string())
                    .or_else(|| last_repo.clone())
                    .or_else(|| fallback.clone());
                if let Some(repo) = before {
                    let key = format!("{repo}#{}", &c[1]);
                    // A Markdown link names the same one twice.
                    let linked = found.iter().any(|f| f.2.ends_with(&format!("#{}", &c[1])));
                    if !linked {
                        push(&mut found, m.start(), m.end(), key);
                    }
                }
            }
            found.sort_by_key(|f| f.0);
            if let Some(f) = found.last() {
                last_repo = f.2.split_once('#').map(|(r, _)| r.to_string());
            }
            let release = RELEASE_WAIT.is_match(sentence);
            let spans: Vec<(usize, usize)> = found.iter().map(|f| (f.0, f.1)).collect();
            for (at, end, key) in found {
                let reference = Reference {
                    key,
                    said: said(sentence, at, end, &spans),
                    awaits_release: release || {
                        let from = floor(sentence, at.saturating_sub(24));
                        let to = floor(sentence, (end + 24).min(sentence.len()));
                        UNRELEASED.is_match(&sentence[from..to])
                    },
                };
                match out.references.iter().position(|r| r.key == reference.key) {
                    Some(i) => {
                        let r = &mut out.references[i];
                        r.awaits_release |= reference.awaits_release;
                        // A memory that records it as done knows it is,
                        // whatever it said while it waited.
                        if r.said != Some(Said::Done) && reference.said.is_some() {
                            r.said = reference.said;
                        }
                        last = Some(i);
                    }
                    None => {
                        out.references.push(reference);
                        last = Some(out.references.len() - 1);
                    }
                }
            }
            // "It isn't in a release yet" names no PR: it is about the one
            // before it.
            if release && let Some(i) = last {
                out.references[i].awaits_release = true;
            }
            // "jdx skipped `jouet mcp`" knows it is not there.
            let absent = ABSENT.is_match(sentence);
            for span in sentence.split('`').skip(1).step_by(2) {
                let words: Vec<&str> = span.split_whitespace().collect();
                if words.first() == Some(&"jouet")
                    && !absent
                    && words.len() > 1
                    && !span.contains(['<', '>', '|', '[', '{', '…', '$', '*'])
                    && !span.contains("...")
                {
                    // `--outline/--symbol` names two flags.
                    let words: Vec<String> = words[1..]
                        .iter()
                        .flat_map(|w| {
                            w.split('/')
                                .filter(|p| p.starts_with("--"))
                                .map(str::to_string)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .chain((!w.starts_with("--")).then(|| (*w).to_string()))
                        })
                        .collect();
                    if !out.commands.contains(&words) {
                        out.commands.push(words);
                    }
                }
                if rules.iter().any(|r| r == span) && OVERRIDE.is_match(sentence) {
                    let entry = (span.to_string(), sentence.clone());
                    if !out.overrides.iter().any(|o| o.0 == entry.0) {
                        out.overrides.push(entry);
                    }
                }
            }
        }
    }
    out
}

/// What GitHub says of a PR or issue now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lookup {
    pub checked: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pr: bool,
    /// OPEN, MERGED or CLOSED; for an issue, CLOSED with its reason;
    /// MISSING when GitHub has none.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When it was merged or closed, as a date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The release that has the merge, once one does: its tag and date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<(String, String)>,
    /// The repository's latest release when last asked, so a merge is
    /// compared with a release only once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compared: Option<String>,
}

impl Lookup {
    /// A PR or issue GitHub did not find.
    fn missing(now: u64) -> Self {
        Self {
            checked: now,
            pr: false,
            state: "MISSING".into(),
            reason: None,
            at: None,
            commit: None,
            released: None,
            compared: None,
        }
    }

    /// Whether asking again could change it.
    fn settled(&self, awaits_release: bool) -> bool {
        match self.state.as_str() {
            "OPEN" => false,
            "MERGED" => !awaits_release || self.released.is_some(),
            _ => true,
        }
    }
}

/// What upkeep keeps in the winding state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Memories {
    #[serde(default)]
    pub next_id: u64,
    /// When the stop hook last started a check.
    #[serde(default)]
    pub last_check: u64,
    #[serde(default)]
    pub notes: Vec<MemoryNote>,
    /// GitHub's answers, by `owner/name#number`.
    #[serde(default)]
    pub lookups: BTreeMap<String, Lookup>,
}

impl Memories {
    pub(crate) fn note(&self, id: &str) -> Option<&MemoryNote> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub(crate) fn set_status(&mut self, id: &str, status: Status) -> bool {
        match self.notes.iter_mut().find(|n| n.id == id) {
            Some(n) => {
                n.status = status;
                true
            }
            None => false,
        }
    }
}

/// What a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    /// A PR or issue it waits on is merged or closed.
    Resolved,
    /// A merge it says is unreleased is in a release.
    Released,
    /// A jouet command or flag it names is gone.
    Gone,
    /// It works around a built-in rule: a jouet bug, not a memory.
    Jouet,
}

/// Something stale in one memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    pub memory: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub kind: Kind,
    /// What it is about, such as `jdx/mbx#886` or `jouet wind nope`; with
    /// the kind, what keeps one finding from being raised twice.
    pub about: String,
    /// What was found, as a clause: "waits on jdx/mbx#886, which was merged
    /// on 2026-10-31".
    pub found: String,
}

/// A finding recorded to show at session starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MemoryNote {
    pub id: String,
    pub created: u64,
    pub memory: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub kind: Kind,
    pub about: String,
    pub found: String,
    /// The idea `jouet share --idea` drafts, for a memory that works
    /// around jouet, when contributing is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idea: Option<String>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

/// One memory's claims, with what the check needs to resolve them.
pub(crate) struct Checked<'a> {
    pub memory: &'a Memory,
    pub claims: Claims,
}

/// The claims of every memory: a project's own repository resolves its
/// bare references.
pub(crate) fn extract<'a>(memories: &'a [Memory], rules: &[String]) -> Vec<Checked<'a>> {
    let mut repos: BTreeMap<String, Option<String>> = BTreeMap::new();
    memories
        .iter()
        .map(|memory| {
            let repo = memory.project.as_ref().and_then(|p| {
                repos
                    .entry(p.clone())
                    .or_insert_with(|| project_dir(p).and_then(|d| origin_repo(&d)))
                    .clone()
            });
            Checked {
                memory,
                claims: claims(&memory.body, repo.as_deref(), rules),
            }
        })
        .collect()
}

/// The references worth asking GitHub about: those a memory waits on,
/// whose answer could still change. `true` when a release is awaited.
pub(crate) fn to_look_up(
    checked: &[Checked<'_>],
    known: &BTreeMap<String, Lookup>,
    now: u64,
) -> BTreeMap<String, bool> {
    let mut out: BTreeMap<String, bool> = BTreeMap::new();
    for c in checked {
        for r in &c.claims.references {
            if r.said != Some(Said::Waiting) && !r.awaits_release {
                continue;
            }
            // A release never compared is asked about now, however
            // recently the merge was.
            let fresh = known.get(&r.key).is_some_and(|l| {
                l.settled(r.awaits_release)
                    || (now.saturating_sub(l.checked) < RECHECK
                        && (!r.awaits_release || l.compared.is_some()))
            });
            if !fresh {
                *out.entry(r.key.clone()).or_default() |= r.awaits_release;
            }
        }
    }
    out
}

/// Asks GitHub about `wanted` (each `owner/name#number`, with whether a
/// release is awaited), in batched GraphQL queries, then compares each
/// awaited merge with its repository's latest release.
pub(crate) fn look_up(
    client: &mut jouet_github::Client,
    wanted: &BTreeMap<String, bool>,
    known: &BTreeMap<String, Lookup>,
    now: u64,
) -> Result<BTreeMap<String, Lookup>, String> {
    let mut by_repo: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for key in wanted.keys() {
        if let Some((repo, n)) = key.split_once('#')
            && let Ok(n) = n.parse()
            && valid_repo(repo)
        {
            by_repo.entry(repo).or_default().push(n);
        }
    }
    let mut out = BTreeMap::new();
    let mut latest: BTreeMap<String, (String, String)> = BTreeMap::new();
    let pairs: Vec<(&str, u64)> = by_repo
        .iter()
        .flat_map(|(repo, ns)| ns.iter().map(move |n| (*repo, *n)))
        .collect();
    for chunk in pairs.chunks(PER_QUERY) {
        let mut repos: Vec<&str> = chunk.iter().map(|p| p.0).collect();
        repos.dedup();
        let mut query = String::from("query {");
        for (i, repo) in repos.iter().enumerate() {
            let (owner, name) = repo.split_once('/').expect("validated");
            query.push_str(&format!(
                " r{i}: repository(owner: {}, name: {}) {{ latestRelease {{ tagName publishedAt }}",
                serde_json::to_string(owner).expect("a string"),
                serde_json::to_string(name).expect("a string"),
            ));
            for (_, n) in chunk.iter().filter(|p| p.0 == *repo) {
                query.push_str(&format!(
                    " n{n}: issueOrPullRequest(number: {n}) {{ __typename ... on PullRequest {{ state mergedAt closedAt mergeCommit {{ oid }} }} ... on Issue {{ state stateReason closedAt }} }}"
                ));
            }
            query.push_str(" }");
        }
        query.push_str(" }");
        let data = client.graphql(&query, &serde_json::json!({}))?;
        for (i, repo) in repos.iter().enumerate() {
            let r = data.get(format!("r{i}")).filter(|r| !r.is_null());
            if let Some(rel) = r
                .and_then(|r| r.get("latestRelease"))
                .filter(|v| !v.is_null())
            {
                let tag = rel["tagName"].as_str().unwrap_or_default().to_string();
                let date = rel["publishedAt"].as_str().unwrap_or_default();
                latest.insert((*repo).to_string(), (tag, date.chars().take(10).collect()));
            }
            for (_, n) in chunk.iter().filter(|p| p.0 == *repo) {
                let key = format!("{repo}#{n}");
                let Some(item) = r
                    .and_then(|r| r.get(format!("n{n}")))
                    .filter(|v| !v.is_null())
                else {
                    // No such repository or number, or not one this token
                    // can see: remembered, so it is not asked again.
                    out.insert(key, Lookup::missing(now));
                    continue;
                };
                let date = |k: &str| item[k].as_str().map(|s| s.chars().take(10).collect());
                let previous = known.get(&key);
                out.insert(
                    key,
                    Lookup {
                        checked: now,
                        pr: item["__typename"] == "PullRequest",
                        state: item["state"].as_str().unwrap_or_default().to_string(),
                        reason: item["stateReason"].as_str().map(str::to_string),
                        at: date("mergedAt").or_else(|| date("closedAt")),
                        commit: item["mergeCommit"]["oid"].as_str().map(str::to_string),
                        released: previous.and_then(|p| p.released.clone()),
                        compared: previous.and_then(|p| p.compared.clone()),
                    },
                );
            }
        }
    }
    for (key, lookup) in &mut out {
        let awaits = wanted.get(key).copied().unwrap_or(false);
        let repo = key.split_once('#').map_or("", |k| k.0);
        let (Some(commit), Some((tag, date))) = (&lookup.commit, latest.get(repo)) else {
            continue;
        };
        if !awaits || lookup.released.is_some() || lookup.compared.as_ref() == Some(tag) {
            continue;
        }
        let path = format!("/repos/{repo}/compare/{tag}...{commit}");
        let Some(compare) = client.rest_get(&path)? else {
            continue;
        };
        lookup.compared = Some(tag.clone());
        // The merge is behind the tag (or is it): the release has it.
        if matches!(compare["status"].as_str(), Some("behind" | "identical")) {
            lookup.released = Some((tag.clone(), date.clone()));
        }
    }
    Ok(out)
}

/// What is stale in each memory, given GitHub's answers. At most one
/// finding of each kind per memory.
pub(crate) fn findings(
    checked: &[Checked<'_>],
    lookups: &BTreeMap<String, Lookup>,
    unknown: &dyn Fn(&[&str]) -> Option<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for c in checked {
        let memory = c.memory;
        let mut push = |kind: Kind, about: String, found: String| {
            if !out
                .iter()
                .any(|f: &Finding| f.path == memory.path.to_string_lossy() && f.kind == kind)
            {
                out.push(Finding {
                    memory: memory.name.clone(),
                    path: memory.path.to_string_lossy().into_owned(),
                    project: memory.project.clone(),
                    kind,
                    about,
                    found,
                });
            }
        };
        for r in &c.claims.references {
            let Some(l) = lookups.get(&r.key) else {
                continue;
            };
            let what = if l.pr { "PR" } else { "issue" };
            let on =
                l.at.as_deref()
                    .map(|d| format!(" on {d}"))
                    .unwrap_or_default();
            if r.awaits_release
                && let Some((tag, date)) = &l.released
            {
                push(
                    Kind::Released,
                    r.key.clone(),
                    format!(
                        "waits for a release with {}, and {tag} ({date}) has it",
                        r.key
                    ),
                );
                continue;
            }
            // What it waits for is a release, not the merge.
            if r.said != Some(Said::Waiting) || r.awaits_release {
                continue;
            }
            let now = match (l.state.as_str(), l.reason.as_deref()) {
                ("MERGED", _) => format!("which was merged{on}"),
                ("CLOSED", Some("NOT_PLANNED")) => format!("which was closed as not planned{on}"),
                ("CLOSED", Some("DUPLICATE")) => format!("which was closed as a duplicate{on}"),
                ("CLOSED", _) if l.pr => format!("which was closed without merging{on}"),
                ("CLOSED", _) => format!("which was closed{on}"),
                _ => continue,
            };
            push(
                Kind::Resolved,
                r.key.clone(),
                format!("waits on {what} {}, {now}", r.key),
            );
        }
        for words in &c.claims.commands {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            if let Some(missing) = unknown(&words) {
                push(
                    Kind::Gone,
                    missing.clone(),
                    format!("names `{missing}`, which this jouet does not have"),
                );
            }
        }
        for (rule, _) in &c.claims.overrides {
            push(
                Kind::Jouet,
                rule.clone(),
                format!("seems to work around jouet's built-in rule `{rule}`"),
            );
        }
    }
    out
}

/// Records new findings as memory notes, within the daily limit: a finding
/// already noted, in any status, is never noted again.
pub(crate) fn record(
    state: &mut State,
    found: &[Finding],
    ideas: &BTreeMap<String, String>,
    now: u64,
) -> Vec<String> {
    let m = &mut state.memories;
    let today = jouet_output::time::date(now);
    let mut budget = MAX_NEW_PER_DAY.saturating_sub(
        m.notes
            .iter()
            .filter(|n| jouet_output::time::date(n.created) == today)
            .count(),
    );
    let mut recorded = Vec::new();
    for f in found {
        if budget == 0 {
            break;
        }
        let path = tilde(Path::new(&f.path));
        if m.notes
            .iter()
            .any(|n| n.path == path && n.kind == f.kind && n.about == f.about)
        {
            continue;
        }
        m.next_id += 1;
        let id = format!("m{}", m.next_id);
        m.notes.push(MemoryNote {
            id: id.clone(),
            created: now,
            memory: f.memory.clone(),
            path,
            project: f.project.clone(),
            kind: f.kind,
            about: f.about.clone(),
            found: f.found.clone(),
            idea: ideas.get(&f.path).cloned(),
            status: Status::Pending,
            shown: Vec::new(),
        });
        recorded.push(id);
        budget -= 1;
    }
    recorded
}

/// Whether the stop hook should start a check now, remembering that it did.
pub(crate) fn due(paths: &Paths, now: u64) -> bool {
    super::update(&paths.state, |state| {
        if now.saturating_sub(state.memories.last_check) < EVERY {
            return false;
        }
        state.memories.last_check = now;
        true
    })
    .unwrap_or(false)
}

/// Starts `jouet wind memories --record` in the background, detached from
/// the hook, which cannot wait for GitHub.
pub(crate) fn spawn_check() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["wind", "memories", "--record"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group, so a harness stopping the hook's group
        // leaves it running.
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

/// The lines to show a session starting in `cwd`, at most `room`: notes
/// about its project's memories and the global ones.
pub(crate) fn lines_for_session(
    paths: &Paths,
    session: &str,
    cwd: Option<&Path>,
    room: usize,
) -> Vec<String> {
    if room == 0 {
        return Vec::new();
    }
    super::update(&paths.state, |state| {
        let mut lines = Vec::new();
        for n in state.memories.notes.iter_mut() {
            if lines.len() == room {
                break;
            }
            if n.status != Status::Pending {
                continue;
            }
            let ours = match (&n.project, cwd) {
                (None, _) => true,
                (Some(p), Some(cwd)) => in_project(p, cwd),
                (Some(_), None) => false,
            };
            if !ours {
                continue;
            }
            super::review::mark_shown(&mut n.shown, &mut n.status, session);
            lines.push(line(n));
        }
        lines
    })
    .unwrap_or_default()
}

/// A memory note as a session start shows it: one line.
pub(crate) fn line(n: &MemoryNote) -> String {
    let what = match n.kind {
        Kind::Resolved | Kind::Released => {
            "the bug it works around may be fixed. At a natural pause, check whether it still happens, then ask the user before updating or deleting the memory"
        }
        Kind::Gone => {
            "it may be out of date. At a natural pause, check `jouet agent-help`, then ask the user before updating or deleting the memory"
        }
        Kind::Jouet => {
            "a jouet bug belongs with jouet's maintainers, not in a memory. Tell the user, and ask before deleting the memory"
        }
    };
    let idea = match &n.idea {
        Some(key) => format!(
            " `jouet share --idea {key}` drafts a report; post it with `--yes` only if they agree."
        ),
        None => String::new(),
    };
    format!(
        "jouet wind: memory `{}` {}: {what}.{idea} `jouet wind show {}` for its path; `jouet wind adopt {}` once done, `jouet wind dismiss {}` to drop it.",
        n.memory, n.found, n.id, n.id, n.id
    )
}

/// The memories a review of sessions in `projects` sees: theirs and the
/// global ones.
pub(crate) fn for_projects<'a>(memories: &'a [Memory], projects: &[PathBuf]) -> Vec<&'a Memory> {
    memories
        .iter()
        .filter(|m| match &m.project {
            None => true,
            Some(p) => projects.iter().any(|dir| in_project(p, dir)),
        })
        .collect()
}

/// A memory as a review sees it.
#[derive(Debug, Serialize)]
pub(crate) struct Shown {
    pub path: String,
    pub name: String,
    pub description: String,
    pub origin: Origin,
    /// Its whole text, redacted: for a memory the session read, or one a
    /// memory note names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Characters of memory index a review shows, and of each memory's text.
const INDEX_CHARS: usize = 6000;
const TEXT_CHARS: usize = 3000;
/// Memories shown whole, at most.
const WHOLE: usize = 3;

/// The memories a review sees, most relevant first: those the sessions
/// read (`read` holds the files the digests list) and those with pending
/// notes, in full; the rest as an index. Redacted with `redact`.
pub(crate) fn for_review(
    memories: &[&Memory],
    read: &BTreeSet<String>,
    noted: &BTreeSet<String>,
    redact: &dyn Fn(&str) -> String,
) -> Vec<Shown> {
    let mut whole = 0;
    let mut out: Vec<Shown> = memories
        .iter()
        .map(|m| {
            let path = m.display();
            let absolute = m.path.to_string_lossy();
            let was_read = read.iter().any(|r| *r == path || *r == absolute);
            let full = (was_read || noted.contains(&path)) && whole < WHOLE;
            if full {
                whole += 1;
            }
            Shown {
                path,
                name: redact(&m.name),
                description: redact(&m.description),
                origin: m.origin,
                text: full.then(|| cut(&redact(&m.body), TEXT_CHARS)),
            }
        })
        .collect();
    out.sort_by_key(|s| s.text.is_none());
    out
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let at = floor(text, max);
    format!(
        "{}\n… {} more characters (read the file)",
        &text[..at],
        text.len() - at
    )
}

/// The memories section a review prints after the digests.
pub(crate) fn render(shown: &[Shown]) -> String {
    use std::fmt::Write as _;
    if shown.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "\n## Memories\n\nThe memories these sessions' agent reads, read-only. To change one, propose `memory_update` or `memory_delete` with `target` set to its path.\n\n",
    );
    let mut used = 0;
    let mut left = 0;
    for s in shown {
        let line = format!("- {} — {}: {}\n", s.path, s.name, s.description);
        if used + line.len() > INDEX_CHARS && s.text.is_none() {
            left += 1;
            continue;
        }
        used += line.len();
        text.push_str(&line);
    }
    if left > 0 {
        let _ = writeln!(
            text,
            "… {left} more memories left out (list their folders to see them)"
        );
    }
    for s in shown {
        if let Some(body) = &s.text {
            let _ = write!(text, "\n### {}\n\n{}\n", s.path, body.trim_end());
        }
    }
    text
}

#[cfg(test)]
mod tests;
