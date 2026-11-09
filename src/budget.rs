//! The context budget `jouet setup claude` offers, and writes only when
//! accepted: a compaction window for sessions on a 1M-token model, and
//! denials for the definitions this user's own transcripts show unused.
//!
//! Both come from the newest [`RECENT`] main transcripts under Claude Code's
//! `projects/` that record their tool definitions (the first
//! `prompt_snapshot` attachment). A tool, MCP server or skill is unused when
//! it was offered in at least [`MIN_SESSIONS`] of them and called in none.
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
/// Tools never offered for denial, used or not: the ones every session
/// works with, and the ones that reach the rest (skills, deferred tools,
/// subagents, questions to the person, plan mode).
const KEEP: &[&str] = &[
    "Bash",
    "Read",
    "Edit",
    "Write",
    "MultiEdit",
    "Grep",
    "Glob",
    "Skill",
    "ToolSearch",
    "Agent",
    "Task",
    "AskUserQuestion",
    "ExitPlanMode",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Tool,
    Mcp,
    Skill,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Mcp => "MCP server",
            Self::Skill => "skill",
        }
    }
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

    pub(crate) fn line(&self) -> String {
        format!(
            "{:>6}  {} ({})",
            format!("~{}", tokens(self.tokens)),
            self.name,
            self.kind.label()
        )
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
    let sessions = files
        .iter()
        .filter_map(|(_, path)| read_session(path))
        .take(RECENT);
    summarize(sessions)
}

fn summarize(sessions: impl Iterator<Item = Session>) -> History {
    let mut history = History::default();
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
                && match kind {
                    Kind::Tool => {
                        !KEEP.contains(&name.as_str())
                            && !crate::setup::UNUSED_CLAUDE_TOOLS.contains(&name.as_str())
                            && !used.contains(&(*kind, name.clone()))
                    }
                    Kind::Mcp => !used.contains(&(*kind, name.clone())),
                    Kind::Skill => !skill_used(name),
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
    let file = std::fs::File::open(path).ok()?;
    let mut session = Session::default();
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
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
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
                if let Some(usage) = message.get("usage") {
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
    session.tools.is_some().then_some(session)
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
}

/// The budget changes for one run of setup.
#[derive(Debug, Default)]
pub(crate) struct Change {
    pub(crate) window: Option<u64>,
    pub(crate) deny: Vec<Unused>,
    pub(crate) undo_window: bool,
    pub(crate) undo_unused: bool,
}

impl Change {
    pub(crate) fn is_empty(&self) -> bool {
        self.window.is_none() && self.deny.is_empty() && !self.undo_window && !self.undo_unused
    }
}

/// Applies `change` to a Claude Code settings document, updating `added`.
/// Returns notes for what it left alone.
pub(crate) fn apply(doc: &mut Value, change: &Change, added: &mut Added) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(object) = doc.as_object_mut() else {
        return notes;
    };
    if change.undo_window
        && let Some(window) = added.auto_compact_window.take()
        && object.get("autoCompactWindow").and_then(Value::as_u64) == Some(window)
    {
        object.remove("autoCompactWindow");
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

    fn session(tools: &[(&str, usize)], called: &[&str], skills: &[&str], peak: u64) -> Session {
        Session {
            tools: Some(tools.iter().map(|(n, c)| (n.to_string(), *c)).collect()),
            skills: skills.iter().map(|n| (n.to_string(), 360)).collect(),
            called: called.iter().map(|n| n.to_string()).collect(),
            skills_called: HashSet::new(),
            peak,
        }
    }

    #[test]
    fn an_item_no_recent_session_called_is_unused() {
        let tools = [
            ("Bash", 4600),
            ("Workflow", 9200),
            ("ScheduleWakeup", 4600),
            ("Artifact", 46000),
            ("mcp__visualize__show_widget", 2300),
            ("mcp__visualize__read_me", 2300),
            ("mcp__ccd_pr__get_status", 2300),
        ];
        let sessions = (0..MIN_SESSIONS).map(|i| {
            let called: &[&str] = if i == 0 {
                &["Bash", "mcp__ccd_pr__get_status"]
            } else {
                &["Bash"]
            };
            session(&tools, called, &["pr-description", "docx"], 150_000)
        });
        let history = summarize(sessions);
        let names: Vec<_> = history
            .unused
            .iter()
            .map(|u| (u.kind, u.name.as_str(), u.tokens))
            .collect();
        assert_eq!(
            names,
            [
                (Kind::Tool, "Workflow", 2000),
                (Kind::Tool, "ScheduleWakeup", 1000),
                (Kind::Mcp, "visualize", 1000),
                (Kind::Skill, "docx", 100),
                (Kind::Skill, "pr-description", 100),
            ]
        );
        assert_eq!(history.past_window, 0);
    }

    #[test]
    fn too_few_sessions_offer_nothing_but_still_show_the_window() {
        let history =
            summarize((1..MIN_SESSIONS).map(|_| session(&[("Workflow", 9200)], &[], &[], 640_000)));
        assert!(history.unused.is_empty());
        assert_eq!(history.past_window, MIN_SESSIONS - 1);
        assert_eq!(history.peak, 640_000);
    }

    #[test]
    fn a_skill_called_or_typed_is_used() {
        let mut sessions: Vec<Session> = (0..MIN_SESSIONS)
            .map(|_| session(&[], &[], &["anthropic-skills:docx", "loop", "simplify"], 0))
            .collect();
        sessions[0].skills_called.insert("docx".into());
        sessions[1].skills_called.insert("loop".into());
        let history = summarize(sessions.into_iter());
        let names: Vec<_> = history.unused.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["simplify"]);
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
