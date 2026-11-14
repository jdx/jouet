//! The context budget `jouet setup claude` offers, and writes only when
//! accepted: a compaction window for sessions on a 1M-token model, and
//! denials for the definitions this user's own transcripts show unused.
//!
//! Both come from the newest [`RECENT`] main transcripts under Claude Code's
//! `projects/` that record their tool definitions (the first
//! `prompt_snapshot` attachment). A tool, MCP server or skill is unused when
//! it was offered in at least [`MIN_SESSIONS`] of them and called in none.
//! Only what the person added can be unused this way (see [`pickable`]):
//! Claude Code's and the app's own tools answer events a quiet stretch of
//! history never shows, and a skill someone wrote is there to trigger.
//! Its tokens per request are estimated from the size of its definition
//! (tool JSON at about 4.6 characters a token, a skill's listing line at
//! about 3.6), so treat them as ±30%.
//!
//! What setup adds is recorded per settings file in jouet's state directory,
//! so each choice is one undo (`--undo window`, `--undo unused`, or
//! `--uninstall` for both) that removes only what setup added.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead as _;
use std::path::{Path, PathBuf};

/// How many recent sessions the offer is judged on.
pub(crate) const RECENT: usize = 50;
/// The fewest sessions an item must have been offered in before zero calls
/// counts as unused; with fewer sessions than this, nothing is.
pub(crate) const MIN_SESSIONS: usize = 20;
/// The window offered to sessions on a 1M-token model: replaying this
/// machine's sessions at 200k cut input by 45–49% in tokens (38–40% by
/// price). A 200k model already compacts there, so it is offered only when
/// a recent session went past it.
pub(crate) const WINDOW: u64 = 200_000;
/// The range Claude Code accepts for `autoCompactWindow`.
const MIN_WINDOW: u64 = 100_000;
const MAX_WINDOW: u64 = 1_000_000;
/// What a hidden skill's `skillOverrides` entry says: out of the model's
/// listing, still there as `/name` for the person.
const HIDDEN: &str = "user-invocable-only";
/// Whether history may call `name` unused: only what the person added and
/// can as well take away. Everything else is depended on, called or not:
///
/// - Claude Code's and the desktop app's built-in tools (`ReadNotifications`,
///   `ReportFindings`, `ScheduleWakeup`, ...) answer a notification, a
///   review or a wake that fifty quiet sessions need never show.
/// - The app's own MCP servers (`ccd_*`, `terminal`, `visualize`, ...) are
///   how it reaches the session; only a server in `servers` (one the
///   person configured, see [`added_servers`]), a plugin's (`plugin_*`) or
///   a claude.ai connector (`claude_ai_*`, or a UUID in the desktop app)
///   is the person's.
/// - A skill without a plugin prefix is the person's own
///   (`~/.claude/skills`), a project's (`.claude/skills`) or one Claude
///   Code bundles (`loop`, `simplify`); each is there to trigger on its
///   own when its moment comes. Only a plugin's (`plugin:skill`) can go.
pub(crate) fn pickable(kind: Kind, name: &str, servers: &HashSet<String>) -> bool {
    match kind {
        Kind::Tool => false,
        Kind::Mcp => {
            servers.contains(name)
                || name.starts_with("plugin_")
                || name.starts_with("claude_ai_")
                || is_uuid(name)
        }
        Kind::Skill => name.contains(':'),
    }
}

/// `8-4-4-4-12` hex digits: the desktop app's name for a claude.ai
/// connector.
fn is_uuid(name: &str) -> bool {
    let groups: Vec<&str> = name.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The MCP servers the person configured: in Claude Code's `.claude.json`
/// (at the top or for any project) and in the `.mcp.json` of any project
/// it lists or a recent session ran in.
fn added_servers(root: &Path, cwds: &HashSet<String>) -> HashSet<String> {
    let read = |path: &Path| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let mut configs: Vec<Value> = Vec::new();
    // `CLAUDE_CONFIG_DIR/.claude.json`, or `~/.claude.json` beside `~/.claude`.
    let mut files = vec![root.join(".claude.json")];
    if root.file_name().is_some_and(|n| n == ".claude")
        && let Some(home) = root.parent()
    {
        files.push(home.join(".claude.json"));
    }
    // Every project Claude Code has opened, not only where the recent
    // sessions ran: a server configured in a project left alone for a while
    // is still the person's, and a repair must not take its denial back.
    let mut dirs: HashSet<String> = cwds.clone();
    for file in files {
        if let Some(doc) = read(&file) {
            if let Some(projects) = doc.get("projects").and_then(Value::as_object) {
                dirs.extend(projects.keys().cloned());
                configs.extend(projects.values().cloned());
            }
            configs.push(doc);
        }
    }
    configs.extend(
        dirs.iter()
            .filter_map(|dir| read(&Path::new(dir).join(".mcp.json"))),
    );
    configs
        .iter()
        .filter_map(|c| c.get("mcpServers").and_then(Value::as_object))
        .flat_map(|servers| servers.keys().cloned())
        .collect()
}

/// What setup added earlier that [`pickable`] now keeps: the
/// `permissions.deny` entries and the hidden skills to put back.
pub(crate) fn wrong_picks(added: &Added, servers: &HashSet<String>) -> (Vec<String>, Vec<String>) {
    let deny = added
        .deny
        .iter()
        .filter(|entry| match entry.strip_prefix("mcp__") {
            Some(server) => !pickable(Kind::Mcp, server, servers),
            None => !pickable(Kind::Tool, entry, servers),
        })
        .cloned()
        .collect();
    let skills = added
        .skills
        .iter()
        .filter(|name| !pickable(Kind::Skill, name, servers))
        .cloned()
        .collect();
    (deny, skills)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Tool,
    Mcp,
    Skill,
}

/// A definition every request carried and no recent session used.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Unused {
    pub(crate) kind: Kind,
    pub(crate) name: String,
    /// Estimated tokens per request.
    pub(crate) tokens: u64,
    /// Recent sessions that carried it.
    pub(crate) sessions: usize,
}

impl Unused {
    /// Its `permissions.deny` entry; a skill gets a `skillOverrides` entry
    /// instead.
    fn deny_entry(&self) -> Option<String> {
        match self.kind {
            Kind::Tool => Some(self.name.clone()),
            Kind::Mcp => Some(format!("mcp__{}", self.name)),
            Kind::Skill => None,
        }
    }

    /// Whether setup's record says it added this one.
    pub(crate) fn recorded_in(&self, added: &Added) -> bool {
        match self.deny_entry() {
            Some(entry) => added.deny.contains(&entry),
            None => added.skills.contains(&self.name),
        }
    }
}

/// What the recent sessions show.
#[derive(Debug, Default, Serialize)]
pub(crate) struct History {
    /// Sessions read, the newest first, up to [`RECENT`].
    pub(crate) sessions: usize,
    /// Of those, the ones whose context went past [`WINDOW`].
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) past_window: usize,
    /// The largest context any of them reached.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub(crate) peak: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) unused: Vec<Unused>,
    /// The MCP servers the person configured, from [`added_servers`].
    #[serde(skip)]
    pub(crate) servers: HashSet<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

/// `1.2k` for 1,234 tokens, `800` below a thousand.
pub(crate) fn tokens(n: u64) -> String {
    if n >= 1000 && n.is_multiple_of(1000) {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Reads `TOKENS` as `--compact-window` takes it: `200k`, `200000` or `1m`,
/// within the range Claude Code accepts.
pub(crate) fn parse_window(text: &str) -> Result<u64, String> {
    let lower = text.trim().to_ascii_lowercase();
    let (digits, scale) = if let Some(d) = lower.strip_suffix('k') {
        (d, 1_000)
    } else if let Some(d) = lower.strip_suffix('m') {
        (d, 1_000_000)
    } else {
        (lower.as_str(), 1)
    };
    let n = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(|| {
            format!("--compact-window takes a token count such as 200k, not `{text}`")
        })?;
    if !(MIN_WINDOW..=MAX_WINDOW).contains(&n) {
        return Err(format!(
            "--compact-window must be from 100k to 1m tokens, which is what Claude Code accepts, not {n}"
        ));
    }
    Ok(n)
}

/// One transcript's definitions, calls and context.
#[derive(Default)]
struct Session {
    /// Inline tools and their definitions' sizes in characters; `None`
    /// until a snapshot is seen.
    tools: Option<Vec<(String, usize)>>,
    /// Skills in the listing, and their lines' sizes.
    skills: Vec<(String, usize)>,
    called: HashSet<String>,
    skills_called: HashSet<String>,
    peak: u64,
    /// Where it ran.
    cwd: Option<String>,
}

/// Reads the newest [`RECENT`] main transcripts under `root/projects` that
/// record their tool definitions.
pub(crate) fn scan(root: &Path) -> History {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let projects = std::fs::read_dir(root.join("projects"))
        .into_iter()
        .flatten();
    for project in projects.flatten() {
        // Only a project directory's own files are main sessions; subagents
        // write theirs a level down.
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl")
                && let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            {
                files.push((modified, path));
            }
        }
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let sessions: Vec<Session> = files
        .iter()
        .filter_map(|(_, path)| read_session(path))
        .take(RECENT)
        .collect();
    let cwds = sessions.iter().filter_map(|s| s.cwd.clone()).collect();
    summarize(sessions.into_iter(), added_servers(root, &cwds))
}

fn summarize(sessions: impl Iterator<Item = Session>, servers: HashSet<String>) -> History {
    let mut history = History {
        servers,
        ..History::default()
    };
    // (kind, name) -> (characters over every session it was in, sessions)
    let mut present: HashMap<(Kind, String), (usize, usize)> = HashMap::new();
    let mut used: HashSet<(Kind, String)> = HashSet::new();
    for session in sessions {
        history.sessions += 1;
        history.peak = history.peak.max(session.peak);
        if session.peak > WINDOW {
            history.past_window += 1;
        }
        let mut this: HashMap<(Kind, String), usize> = HashMap::new();
        for (name, chars) in session.tools.unwrap_or_default() {
            let key = match mcp_server(&name) {
                Some(server) => (Kind::Mcp, server.to_string()),
                None => (Kind::Tool, name),
            };
            *this.entry(key).or_default() += chars;
        }
        for (name, chars) in session.skills {
            *this.entry((Kind::Skill, name)).or_default() += chars;
        }
        for (key, chars) in this {
            let entry = present.entry(key).or_default();
            entry.0 += chars;
            entry.1 += 1;
        }
        for name in session.called {
            used.insert(match mcp_server(&name) {
                Some(server) => (Kind::Mcp, server.to_string()),
                None => (Kind::Tool, name),
            });
        }
        for name in session.skills_called {
            used.insert((Kind::Skill, name));
        }
    }
    if history.sessions < MIN_SESSIONS {
        return history;
    }
    let skill_used = |name: &str| {
        // A plugin's skill may be called by its name alone.
        let short = name.rsplit_once(':').map_or(name, |(_, s)| s);
        used.contains(&(Kind::Skill, name.to_string()))
            || used.contains(&(Kind::Skill, short.to_string()))
    };
    let mut unused: Vec<Unused> = present
        .into_iter()
        .filter(|((kind, name), (_, n))| {
            *n >= MIN_SESSIONS
                && pickable(*kind, name, &history.servers)
                && match kind {
                    Kind::Skill => !skill_used(name),
                    _ => !used.contains(&(*kind, name.clone())),
                }
        })
        .map(|((kind, name), (chars, n))| {
            let per_token = if kind == Kind::Skill { 3.6 } else { 4.6 };
            Unused {
                kind,
                name,
                tokens: (chars as f64 / n as f64 / per_token).round() as u64,
                sessions: n,
            }
        })
        .filter(|u| u.tokens > 0)
        .collect();
    unused.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.name.cmp(&b.name)));
    history.unused = unused;
    history
}

/// `server` for an MCP tool's `mcp__server__tool`.
fn mcp_server(tool: &str) -> Option<&str> {
    let rest = tool.strip_prefix("mcp__")?;
    Some(rest.split_once("__").map_or(rest, |(server, _)| server))
}

/// One transcript, or `None` without a tool snapshot (an older Claude Code,
/// or not a session).
fn read_session(path: &Path) -> Option<Session> {
    let mut session = Session::default();
    read_into(path, &mut session, true).ok()?;
    session.tools.as_ref()?;
    // What the session's subagents called counts as used too: a denial
    // reaches them as well.
    for sub in subagent_transcripts(&path.with_extension("")) {
        let _ = read_into(&sub, &mut session, false);
    }
    Some(session)
}

/// The transcripts under a session's own directory (`subagents/*.jsonl`),
/// at any depth.
fn subagent_transcripts(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                found.push(path);
            }
        }
    }
    found
}

/// Reads one transcript into `session`. From a subagent's transcript, or a
/// sidechain record in the session's own, only the calls count: the
/// definitions and context are the main conversation's.
fn read_into(path: &Path, session: &mut Session, main: bool) -> std::io::Result<()> {
    let file = std::fs::File::open(path)?;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else {
            break;
        };
        // Parsing every line would read every tool result; only these
        // records matter.
        let snapshot = session.tools.is_none() && line.contains("\"prompt_snapshot\"");
        let listing = session.skills.is_empty() && line.contains("\"skill_listing\"");
        let tool_use = line.contains("\"tool_use\"");
        let usage = line.contains("\"usage\"");
        let command = line.contains("<command-name>");
        if !(snapshot || listing || tool_use || usage || command) {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let side = !main || record.get("isSidechain").and_then(Value::as_bool) == Some(true);
        if session.cwd.is_none() && !side {
            session.cwd = record.get("cwd").and_then(Value::as_str).map(String::from);
        }
        let (snapshot, listing) = (snapshot && !side, listing && !side);
        let attachment = record.get("attachment");
        let kind = attachment
            .and_then(|a| a.get("type"))
            .and_then(Value::as_str);
        if snapshot
            && kind == Some("prompt_snapshot")
            && let Some(tools) = attachment
                .and_then(|a| a.get("tools"))
                .and_then(Value::as_array)
        {
            session.tools = Some(
                tools
                    .iter()
                    .filter_map(|tool| {
                        let name = tool.get("name")?.as_str()?.to_string();
                        Some((name, tool.to_string().len()))
                    })
                    .collect(),
            );
        }
        if listing
            && kind == Some("skill_listing")
            && let Some(content) = attachment
                .and_then(|a| a.get("content"))
                .and_then(Value::as_str)
        {
            session.skills = listing_entries(content);
        }
        if let Some(message) = record.get("message") {
            if record.get("type").and_then(Value::as_str) == Some("assistant") {
                if !side && let Some(usage) = message.get("usage") {
                    let field = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let context = field("input_tokens")
                        + field("cache_read_input_tokens")
                        + field("cache_creation_input_tokens");
                    session.peak = session.peak.max(context);
                }
                for block in message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let Some(name) = block.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    if name == "Skill"
                        && let Some(skill) = block
                            .get("input")
                            .and_then(|i| i.get("skill"))
                            .and_then(Value::as_str)
                    {
                        session.skills_called.insert(skill.to_string());
                    }
                    session.called.insert(name.to_string());
                }
            }
            if command {
                // A skill the person typed as `/name`.
                const TAG: &str = "<command-name>/";
                for (at, _) in line.match_indices(TAG) {
                    if let Some((name, _)) = line[at + TAG.len()..].split_once("</command-name>") {
                        session.skills_called.insert(name.to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

/// The skill listing's entries, `- name: description`, each possibly over
/// several lines.
fn listing_entries(content: &str) -> Vec<(String, usize)> {
    let mut entries = Vec::new();
    for entry in content.split("\n- ") {
        let entry = entry.strip_prefix("- ").unwrap_or(entry);
        if let Some((name, _)) = entry.split_once(": ") {
            entries.push((name.trim().to_string(), entry.len() + 3));
        }
    }
    entries
}

/// Whether the settings, or the environment, already choose a compaction
/// window.
pub(crate) fn window_chosen(doc: &Value) -> bool {
    std::env::var_os("CLAUDE_CODE_AUTO_COMPACT_WINDOW").is_some()
        || doc.get("autoCompactWindow").is_some()
        || doc
            .get("modelSettings")
            .and_then(Value::as_object)
            .is_some_and(|models| {
                models
                    .values()
                    .any(|m| m.get("autoCompactWindow").is_some())
            })
}

/// The unused items the settings do not already deny, allow, ask about or
/// override.
pub(crate) fn offered(history: &History, doc: &Value) -> Vec<Unused> {
    history
        .unused
        .iter()
        .filter(|u| match u.deny_entry() {
            Some(entry) => !["deny", "allow", "ask"]
                .iter()
                .any(|key| listed(doc, key, &entry)),
            None => doc
                .get("skillOverrides")
                .and_then(|o| o.get(&u.name))
                .is_none(),
        })
        .cloned()
        .collect()
}

fn listed(doc: &Value, key: &str, entry: &str) -> bool {
    listed_in(doc.get("permissions"), key, entry)
}

fn listed_in(permissions: Option<&Value>, key: &str, entry: &str) -> bool {
    permissions
        .and_then(|p| p.get(key))
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(entry)))
}

/// What one settings file has from setup's budget, recorded so an undo
/// removes only that.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Added {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auto_compact_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) skills: Vec<String>,
    /// `--undo window` was run: a plain setup sets no window again until
    /// `--compact-window` asks for one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) declined_window: bool,
    /// `--undo unused` was run: a plain setup denies nothing again until
    /// `--deny-unused` asks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) declined_unused: bool,
}

/// The budget changes for one run of setup.
#[derive(Debug, Default)]
pub(crate) struct Change {
    pub(crate) window: Option<u64>,
    pub(crate) deny: Vec<Unused>,
    pub(crate) undo_window: bool,
    pub(crate) undo_unused: bool,
    /// The undo was asked for (`--undo`), not part of `--uninstall`, so a
    /// later plain setup leaves that part alone.
    pub(crate) decline: bool,
    /// `--deny-unused` was given, which lifts an earlier `--undo unused`.
    pub(crate) deny_asked: bool,
    /// What setup added earlier and [`wrong_picks`] puts back: denials and
    /// hidden skills.
    pub(crate) restore: (Vec<String>, Vec<String>),
    /// Only for the notes' wording: nothing is written either way.
    pub(crate) dry_run: bool,
}

impl Change {
    pub(crate) fn is_empty(&self) -> bool {
        // `--deny-unused` with nothing to deny still lifts an earlier
        // `--undo unused`, which only the record keeps.
        self.window.is_none()
            && self.deny.is_empty()
            && !self.undo_window
            && !self.undo_unused
            && !self.deny_asked
            && self.restore.0.is_empty()
            && self.restore.1.is_empty()
    }
}

/// Applies `change` to a Claude Code settings document, updating `added`.
/// Returns notes for what it left alone.
pub(crate) fn apply(doc: &mut Value, change: &Change, added: &mut Added) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(object) = doc.as_object_mut() else {
        return notes;
    };
    if change.decline {
        added.declined_window |= change.undo_window;
        added.declined_unused |= change.undo_unused;
    }
    if change.window.is_some() {
        added.declined_window = false;
    }
    if change.deny_asked {
        added.declined_unused = false;
    }
    if change.undo_window
        && let Some(window) = added.auto_compact_window
    {
        // Only the value setup set is setup's to take back; one changed
        // since is yours, and the record of setup's goes either way.
        match object.get("autoCompactWindow") {
            Some(current) if current.as_u64() == Some(window) => {
                object.remove("autoCompactWindow");
                let verb = if change.dry_run {
                    "would remove"
                } else {
                    "removed"
                };
                notes.push(format!(
                    "{verb} the autoCompactWindow of {window} that setup set"
                ));
            }
            Some(current) => notes.push(format!(
                "autoCompactWindow is now {current}, not the {window} setup set, so it is left as it is"
            )),
            None => {}
        }
        added.auto_compact_window = None;
    }
    let (deny, skills) = &change.restore;
    let deny: Vec<String> = deny
        .iter()
        .filter(|d| added.deny.contains(d))
        .cloned()
        .collect();
    let skills: Vec<String> = skills
        .iter()
        .filter(|s| added.skills.contains(s))
        .cloned()
        .collect();
    if !change.undo_unused {
        // Only what is still there as setup left it goes back; a skill
        // override changed since is the person's, and is named as left.
        let mut back: Vec<&str> = deny
            .iter()
            .filter(|d| listed_in(object.get("permissions"), "deny", d))
            .map(String::as_str)
            .collect();
        remove_from(object, "permissions", "deny", |v| {
            v.as_str().is_some_and(|s| deny.iter().any(|d| d == s))
        });
        let mut changed: Vec<&str> = Vec::new();
        if let Some(overrides) = object
            .get_mut("skillOverrides")
            .and_then(Value::as_object_mut)
        {
            for name in &skills {
                match overrides.get(name).map(Value::as_str) {
                    Some(Some(HIDDEN)) => {
                        overrides.remove(name);
                        back.push(name);
                    }
                    Some(_) => changed.push(name),
                    None => {}
                }
            }
            if overrides.is_empty() {
                object.remove("skillOverrides");
            }
        }
        let verb = if change.dry_run {
            "would put back"
        } else {
            "put back"
        };
        if !back.is_empty() {
            notes.push(format!(
                "{verb} {}: setup took them for unused, but they are Claude Code's, the app's or your own",
                back.join(", ")
            ));
        }
        if !changed.is_empty() {
            notes.push(format!(
                "left the skill overrides of {} as they are: they were changed since setup hid them",
                changed.join(", ")
            ));
        }
        added.deny.retain(|d| !deny.contains(d));
        added.skills.retain(|s| !skills.contains(s));
    }
    if change.undo_unused {
        let deny = std::mem::take(&mut added.deny);
        remove_from(object, "permissions", "deny", |v| {
            v.as_str().is_some_and(|s| deny.iter().any(|d| d == s))
        });
        let skills = std::mem::take(&mut added.skills);
        if let Some(overrides) = object
            .get_mut("skillOverrides")
            .and_then(Value::as_object_mut)
        {
            overrides.retain(|name, v| !(skills.contains(name) && v.as_str() == Some(HIDDEN)));
            if overrides.is_empty() {
                object.remove("skillOverrides");
            }
        }
    }
    if let Some(window) = change.window {
        let current = object.get("autoCompactWindow");
        let ours = added
            .auto_compact_window
            .is_some_and(|w| current.and_then(Value::as_u64) == Some(w));
        match current {
            Some(current) if !ours => {
                notes.push(format!(
                    "autoCompactWindow is already {current} in these settings; left as it is"
                ));
            }
            _ => {
                object.insert("autoCompactWindow".into(), json!(window));
                added.auto_compact_window = Some(window);
            }
        }
    }
    for item in &change.deny {
        match item.deny_entry() {
            Some(entry) => {
                if ["deny", "allow", "ask"]
                    .iter()
                    .any(|key| listed_in(object.get("permissions"), key, &entry))
                {
                    continue;
                }
                let permissions = object
                    .entry("permissions")
                    .or_insert_with(|| Value::Object(Map::new()));
                let Some(deny) = permissions
                    .as_object_mut()
                    .map(|p| p.entry("deny").or_insert_with(|| Value::Array(Vec::new())))
                    .and_then(Value::as_array_mut)
                else {
                    notes.push("`permissions.deny` is not a list; nothing was denied".into());
                    break;
                };
                deny.push(json!(entry));
                added.deny.push(entry);
            }
            None => {
                let overrides = object
                    .entry("skillOverrides")
                    .or_insert_with(|| Value::Object(Map::new()));
                let Some(overrides) = overrides.as_object_mut() else {
                    notes.push("`skillOverrides` is not an object; no skill was hidden".into());
                    continue;
                };
                if overrides.contains_key(&item.name) {
                    continue;
                }
                overrides.insert(item.name.clone(), json!(HIDDEN));
                added.skills.push(item.name.clone());
            }
        }
    }
    notes
}

/// Removes the matching values from `object[outer][inner]`, then whatever
/// that left empty.
fn remove_from(
    object: &mut Map<String, Value>,
    outer: &str,
    inner: &str,
    matches: impl Fn(&Value) -> bool,
) {
    let Some(table) = object.get_mut(outer).and_then(Value::as_object_mut) else {
        return;
    };
    let Some(list) = table.get_mut(inner).and_then(Value::as_array_mut) else {
        return;
    };
    let had = list.len();
    list.retain(|v| !matches(v));
    if list.len() == had || !list.is_empty() {
        return;
    }
    table.remove(inner);
    if table.is_empty() {
        object.remove(outer);
    }
}

/// `$XDG_STATE_HOME/jouet/setup-budget.json`: what setup added, keyed by
/// settings file.
fn record_path() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("setup-budget.json"))
}

fn records() -> BTreeMap<String, Added> {
    record_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// What setup's budget added to `settings`.
pub(crate) fn load(settings: &Path) -> Added {
    records()
        .remove(&settings.display().to_string())
        .unwrap_or_default()
}

/// Records what setup's budget has added to `settings`.
pub(crate) fn save(settings: &Path, added: &Added) -> std::io::Result<()> {
    let Some(path) = record_path() else {
        return Ok(());
    };
    let mut all = records();
    let key = settings.display().to_string();
    if *added == Added::default() {
        if all.remove(&key).is_none() {
            return Ok(());
        }
    } else {
        all.insert(key, added.clone());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(&all).map_err(std::io::Error::other)?;
    text.push('\n');
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_unused_alone_is_a_change() {
        assert!(Change::default().is_empty());
        let asked = Change {
            deny_asked: true,
            ..Change::default()
        };
        assert!(!asked.is_empty());
    }

    fn session(tools: &[(&str, usize)], called: &[&str], skills: &[&str], peak: u64) -> Session {
        Session {
            tools: Some(tools.iter().map(|(n, c)| (n.to_string(), *c)).collect()),
            skills: skills.iter().map(|n| (n.to_string(), 360)).collect(),
            called: called.iter().map(|n| n.to_string()).collect(),
            skills_called: HashSet::new(),
            peak,
            cwd: None,
        }
    }

    fn servers(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn only_what_the_person_added_and_no_recent_session_called_is_unused() {
        let tools = [
            ("Bash", 4600),
            // Built in, and never called: still depended on.
            ("Workflow", 9200),
            ("ReadNotifications", 4600),
            ("ReportFindings", 4600),
            // The app's own servers.
            ("mcp__visualize__show_widget", 2300),
            ("mcp__terminal__read_terminal", 2300),
            ("mcp__ccd_pr__get_status", 2300),
            // The person's: configured, a plugin's and connectors.
            ("mcp__linear__list", 4600),
            ("mcp__github__search", 2300),
            ("mcp__plugin_cloudflare_cloudflare__docs", 2300),
            ("mcp__claude_ai_Gmail__search", 2300),
            ("mcp__1a59c906-04da-521d-bda7-7f71b9f9e01c__guide", 2300),
        ];
        let skills = [
            "update-aube",
            "loop",
            "mise-docs-validation",
            "cloudflare:wrangler",
            "anthropic-skills:docx",
        ];
        let sessions = (0..MIN_SESSIONS).map(|i| {
            let called: &[&str] = if i == 0 {
                &["Bash", "mcp__github__search"]
            } else {
                &["Bash"]
            };
            session(&tools, called, &skills, 150_000)
        });
        let history = summarize(sessions, servers(&["linear", "github"]));
        let names: Vec<_> = history
            .unused
            .iter()
            .map(|u| (u.kind, u.name.as_str(), u.tokens))
            .collect();
        assert_eq!(
            names,
            [
                (Kind::Mcp, "linear", 1000),
                (Kind::Mcp, "1a59c906-04da-521d-bda7-7f71b9f9e01c", 500),
                (Kind::Mcp, "claude_ai_Gmail", 500),
                (Kind::Mcp, "plugin_cloudflare_cloudflare", 500),
                (Kind::Skill, "anthropic-skills:docx", 100),
                (Kind::Skill, "cloudflare:wrangler", 100),
            ]
        );
        assert_eq!(history.past_window, 0);
        assert!(!is_uuid("1a59c906-04da-521d-bda7-7f71b9f9e01"));
        assert!(!is_uuid("ccd_pr"));
    }

    #[test]
    fn the_servers_come_from_claude_json_and_where_sessions_ran() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".claude");
        let project = dir.path().join("p");
        // A project Claude Code opened, but no recent session ran in.
        let idle = dir.path().join("idle");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&idle).unwrap();
        std::fs::write(
            dir.path().join(".claude.json"),
            json!({"mcpServers": {"linear": {}},
                   "projects": {"/x": {"mcpServers": {"github": {}}},
                                idle.display().to_string(): {}}})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            project.join(".mcp.json"),
            json!({"mcpServers": {"sentry": {}}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            idle.join(".mcp.json"),
            json!({"mcpServers": {"posthog": {}}}).to_string(),
        )
        .unwrap();
        let cwds = [project.display().to_string(), "/nowhere".into()].into();
        let mut found: Vec<_> = added_servers(&root, &cwds).into_iter().collect();
        found.sort();
        assert_eq!(found, ["github", "linear", "posthog", "sentry"]);
    }

    #[test]
    fn what_an_earlier_run_wrongly_picked_goes_back_and_the_rest_stays() {
        let mut doc = json!({
            "permissions": {"deny": ["Bash(rm:*)", "ReportFindings", "mcp__terminal", "mcp__linear"]},
            "skillOverrides": {"update-aube": HIDDEN, "cloudflare:wrangler": HIDDEN, "loop": "on"}
        });
        let mut added = Added {
            deny: vec![
                "ReportFindings".into(),
                "mcp__terminal".into(),
                "mcp__linear".into(),
            ],
            skills: vec![
                "update-aube".into(),
                "cloudflare:wrangler".into(),
                "loop".into(),
            ],
            ..Added::default()
        };
        let restore = wrong_picks(&added, &servers(&["linear"]));
        assert_eq!(
            restore,
            (
                vec!["ReportFindings".into(), "mcp__terminal".into()],
                vec!["update-aube".into(), "loop".into()]
            )
        );
        let change = Change {
            restore,
            ..Change::default()
        };
        let notes = apply(&mut doc, &change, &mut added);
        assert_eq!(
            doc,
            json!({
                "permissions": {"deny": ["Bash(rm:*)", "mcp__linear"]},
                // `loop` was set to "on" since, so it is the person's now.
                "skillOverrides": {"cloudflare:wrangler": HIDDEN, "loop": "on"}
            })
        );
        assert_eq!(added.deny, ["mcp__linear"]);
        assert_eq!(added.skills, ["cloudflare:wrangler"]);
        assert_eq!(
            notes,
            [
                "put back ReportFindings, mcp__terminal, update-aube: setup took them for unused, \
              but they are Claude Code's, the app's or your own",
                "left the skill overrides of loop as they are: they were changed since setup hid them"
            ]
        );
    }

    #[test]
    fn too_few_sessions_offer_nothing_but_still_show_the_window() {
        let history = summarize(
            (1..MIN_SESSIONS).map(|_| session(&[("mcp__linear__list", 9200)], &[], &[], 640_000)),
            servers(&["linear"]),
        );
        assert!(history.unused.is_empty());
        assert_eq!(history.past_window, MIN_SESSIONS - 1);
        assert_eq!(history.peak, 640_000);
    }

    #[test]
    fn a_skill_called_or_typed_is_used() {
        let mut sessions: Vec<Session> = (0..MIN_SESSIONS)
            .map(|_| {
                session(
                    &[],
                    &[],
                    &[
                        "anthropic-skills:docx",
                        "cloudflare:loop",
                        "cloudflare:simplify",
                    ],
                    0,
                )
            })
            .collect();
        sessions[0].skills_called.insert("docx".into());
        sessions[1].skills_called.insert("cloudflare:loop".into());
        let history = summarize(sessions.into_iter(), HashSet::new());
        let names: Vec<_> = history.unused.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["cloudflare:simplify"]);
    }

    #[test]
    fn the_listing_is_read_entry_by_entry() {
        let entries = listing_entries("- a: one\n- plugin:b: two\nmore\n- c: three");
        let names: Vec<_> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "plugin:b", "c"]);
    }

    #[test]
    fn the_window_takes_k_and_m_within_claude_codes_range() {
        assert_eq!(parse_window("200k"), Ok(200_000));
        assert_eq!(parse_window("1M"), Ok(1_000_000));
        assert_eq!(parse_window("150000"), Ok(150_000));
        assert!(parse_window("50k").is_err());
        assert!(parse_window("2m").is_err());
        assert!(parse_window("big").is_err());
    }

    fn unused(kind: Kind, name: &str) -> Unused {
        Unused {
            kind,
            name: name.into(),
            tokens: 1000,
            sessions: MIN_SESSIONS,
        }
    }

    #[test]
    fn each_choice_is_one_undo_that_keeps_everything_else() {
        let original = json!({
            "permissions": {"deny": ["Bash(rm:*)"], "allow": ["mcp__mine"]},
            "skillOverrides": {"loop": "off"}
        });
        let mut doc = original.clone();
        let mut added = Added::default();
        let change = Change {
            window: Some(WINDOW),
            deny: vec![
                unused(Kind::Tool, "Workflow"),
                unused(Kind::Mcp, "visualize"),
                unused(Kind::Mcp, "mine"),
                unused(Kind::Skill, "docx"),
                unused(Kind::Skill, "loop"),
            ],
            ..Change::default()
        };
        assert!(apply(&mut doc, &change, &mut added).is_empty());
        assert_eq!(
            doc,
            json!({
                "permissions": {
                    "deny": ["Bash(rm:*)", "Workflow", "mcp__visualize"],
                    "allow": ["mcp__mine"]
                },
                "skillOverrides": {"loop": "off", "docx": HIDDEN},
                "autoCompactWindow": 200_000
            })
        );
        assert_eq!(added.deny, ["Workflow", "mcp__visualize"]);
        assert_eq!(added.skills, ["docx"]);

        let undo_window = Change {
            undo_window: true,
            ..Change::default()
        };
        apply(&mut doc, &undo_window, &mut added);
        assert!(doc.get("autoCompactWindow").is_none());
        assert_eq!(added.auto_compact_window, None);
        assert_eq!(doc["permissions"]["deny"].as_array().unwrap().len(), 3);

        let undo_unused = Change {
            undo_unused: true,
            ..Change::default()
        };
        apply(&mut doc, &undo_unused, &mut added);
        assert_eq!(doc, original);
        assert_eq!(added, Added::default());
    }

    #[test]
    fn what_a_subagent_or_sidechain_called_counts_as_used() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let snapshot = json!({"type": "attachment", "attachment": {"type": "prompt_snapshot",
            "tools": [{"name": "Workflow"}, {"name": "ListAgents"}, {"name": "ReportFindings"}]}});
        let sidechain = json!({"type": "assistant", "isSidechain": true, "message": {
            "usage": {"input_tokens": 900_000},
            "content": [{"type": "tool_use", "name": "ListAgents"}]}});
        std::fs::write(&main, format!("{snapshot}\n{sidechain}\n")).unwrap();
        let subagents = dir.path().join("s").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        let call = json!({"type": "assistant", "message": {
            "content": [{"type": "tool_use", "name": "Workflow"}]}});
        std::fs::write(subagents.join("agent-a1.jsonl"), format!("{call}\n")).unwrap();

        let session = read_session(&main).unwrap();
        let mut called: Vec<_> = session.called.iter().map(String::as_str).collect();
        called.sort_unstable();
        assert_eq!(called, ["ListAgents", "Workflow"]);
        // A sidechain's context is not the main conversation's.
        assert_eq!(session.peak, 0);
    }

    #[test]
    fn undoing_a_window_changed_since_leaves_it_and_says_so() {
        let mut doc = json!({"autoCompactWindow": 300_000});
        let mut added = Added {
            auto_compact_window: Some(WINDOW),
            ..Added::default()
        };
        let undo = Change {
            undo_window: true,
            ..Change::default()
        };
        let notes = apply(&mut doc, &undo, &mut added);
        assert_eq!(doc, json!({"autoCompactWindow": 300_000}));
        assert_eq!(
            notes,
            ["autoCompactWindow is now 300000, not the 200000 setup set, so it is left as it is"]
        );
        assert_eq!(added, Added::default());
    }

    #[test]
    fn a_window_the_person_chose_is_left_alone() {
        let mut doc = json!({"autoCompactWindow": 400_000});
        let mut added = Added::default();
        let change = Change {
            window: Some(WINDOW),
            ..Change::default()
        };
        let notes = apply(&mut doc, &change, &mut added);
        assert_eq!(doc, json!({"autoCompactWindow": 400_000}));
        assert_eq!(notes.len(), 1);
        // Nor does an undo take it, since setup did not set it.
        apply(
            &mut doc,
            &Change {
                undo_window: true,
                ..Change::default()
            },
            &mut added,
        );
        assert_eq!(doc, json!({"autoCompactWindow": 400_000}));
    }

    #[test]
    fn what_the_settings_already_name_is_not_offered() {
        let history = History {
            sessions: RECENT,
            unused: vec![
                unused(Kind::Tool, "Workflow"),
                unused(Kind::Tool, "ListAgents"),
                unused(Kind::Mcp, "visualize"),
                unused(Kind::Skill, "docx"),
            ],
            ..History::default()
        };
        let doc = json!({
            "permissions": {"deny": ["Workflow"], "ask": ["mcp__visualize"]},
            "skillOverrides": {"docx": "on"}
        });
        let names: Vec<_> = offered(&history, &doc)
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert_eq!(names, ["ListAgents"]);
        assert!(window_chosen(
            &json!({"modelSettings": {"opus": {"autoCompactWindow": "auto"}}})
        ));
        assert!(!window_chosen(&json!({"modelSettings": {"opus": {}}})));
    }
}
