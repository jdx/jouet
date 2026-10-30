//! Claude Code transcripts: `~/.claude/projects/<project>/<session>.jsonl`,
//! with subagents in `<session>/subagents/agent-*.jsonl`.
//!
//! An assistant message is written as one line per content block, every
//! line carrying the same message ID and usage, so a request is a run of
//! lines with one ID. A tool's duration is the time from the line that
//! called it to the line holding its result.

use super::json::{Flag, Num, Obj, Output, Str};
use super::{Filter, Reader, Source, files_under, home, lines, sources};
use crate::event::{self, Call, MessageKind, Request, Tool, Transcript};
use crate::time::parse_timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::PathBuf;

/// Reads Claude Code's transcripts under a config directory.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    /// `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
    pub root: PathBuf,
}

impl ClaudeCode {
    /// The config directory Claude Code itself uses.
    pub fn from_env() -> Option<Self> {
        let root = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".claude")))?;
        Some(Self { root })
    }
}

/// Claude Code names a project's directory after its path, with every
/// character that is not a letter or digit replaced by `-`.
fn encode(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

impl Reader for ClaudeCode {
    fn harness(&self) -> &'static str {
        "claude"
    }

    fn discover(&self, filter: &Filter) -> io::Result<Vec<Source>> {
        let projects = self.root.join("projects");
        let entries = match std::fs::read_dir(&projects) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        // The encoding is lossy, so it only narrows the directories read;
        // each transcript's own `cwd` decides.
        let prefix = filter.project.as_ref().and_then(|p| p.to_str()).map(encode);
        let mut paths = Vec::new();
        // An entry that vanished or cannot be inspected is skipped, not the
        // whole harness.
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            if let Some(prefix) = &prefix
                && !name.to_string_lossy().starts_with(prefix.as_str())
            {
                continue;
            }
            let _ = files_under(&entry.path(), &[".jsonl"], &mut paths);
        }
        // Only sessions and their subagents (workflow agents included), not
        // workflow journals or other files kept beside them.
        paths.retain(|p| {
            p.parent().and_then(|d| d.parent()) == Some(projects.as_path())
                || subagents_dir(p).is_some()
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("agent-"))
        });
        Ok(sources(paths))
    }

    fn read(
        &self,
        source: &Source,
        input: &mut dyn BufRead,
        filter: &Filter,
    ) -> io::Result<Option<Transcript>> {
        let mut t = Transcript {
            session: session_of(source),
            ..Default::default()
        };
        let mut state = State {
            detail: filter.detail,
            ..State::default()
        };
        let mut other_project = false;
        t.truncated = lines(input, |line| {
            t.lines += 1;
            state.line_no = t.lines;
            let assistant = memchr::memmem::find(line, b"\"type\":\"assistant\"").is_some();
            if !assistant && memchr::memmem::find(line, b"\"type\":\"user\"").is_none() {
                return true;
            }
            let parsed: Line = match serde_json::from_slice(line) {
                Ok(parsed) => parsed,
                Err(_) => {
                    t.bad_lines += 1;
                    return true;
                }
            };
            if t.cwd.is_none()
                && let Some(cwd) = parsed.cwd.0.clone()
            {
                if !filter.matches_cwd(Some(&cwd)) {
                    other_project = true;
                    return false;
                }
                t.cwd = Some(cwd);
            }
            if !state.session_known
                && let Some(id) = &parsed.session_id.0
            {
                t.session.clone_from(id);
                state.session_known = true;
            }
            if state.detail
                && !assistant
                && !parsed.is_meta.0
                && memchr::memmem::find(line, b"\"tool_result\"").is_none()
            {
                state.prompt(&mut t, &parsed, line);
            }
            state.line(&mut t, parsed);
            true
        });
        if other_project || (filter.project.is_some() && t.cwd.is_none()) {
            return Ok(None);
        }
        Ok(Some(t))
    }
}

/// The `subagents` directory a subagent's transcript is under:
/// `<session>/subagents/agent-*.jsonl`, or deeper for workflow agents.
fn subagents_dir(path: &std::path::Path) -> Option<&std::path::Path> {
    path.ancestors()
        .skip(1)
        .find(|d| d.file_name().is_some_and(|n| n == "subagents"))
}

/// A subagent's transcript belongs to its parent session.
fn session_of(source: &Source) -> String {
    let named = subagents_dir(&source.path)
        .and_then(|d| d.parent())
        .unwrap_or(&source.path);
    named
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[derive(Default)]
struct State {
    /// Reading for a digest: see [`Filter::detail`].
    detail: bool,
    /// The line being read, from 1.
    line_no: u64,
    /// The transcript named its session; a subagent's names its parent's.
    session_known: bool,
    /// The last request's message ID.
    message: Option<String>,
    /// Calls waiting for their result, by tool-use ID.
    pending: HashMap<String, usize>,
}

/// What a refused call's result says, before the reason the user gave.
const REFUSED: &str = "The user doesn't want to proceed with this tool use";
const REASON: &str = "the user said:\n";
/// What Claude Code writes in the user's place when they stop a turn.
const INTERRUPTED: &str = "[Request interrupted by user";

impl State {
    /// A user line that holds no tool result: a prompt, an interruption, or
    /// text the harness wrote in the user's place, which is left out.
    fn prompt(&mut self, t: &mut Transcript, parsed: &Line, line: &[u8]) {
        #[derive(Deserialize)]
        struct PromptLine {
            #[serde(default)]
            message: Obj<PromptMessage>,
        }
        #[derive(Deserialize)]
        struct PromptMessage {
            #[serde(default)]
            content: Output,
        }
        let Ok(PromptLine {
            message: Obj(Some(m)),
        }) = serde_json::from_slice(line)
        else {
            return;
        };
        let text = m.content.head.trim();
        let kind = if text.starts_with(INTERRUPTED) {
            MessageKind::Interrupt
        } else if harness_text(text) {
            return;
        } else {
            MessageKind::Prompt
        };
        t.messages.push(event::Message {
            at: parsed.timestamp.as_deref().and_then(parse_timestamp),
            line: self.line_no,
            after_calls: t.calls.len(),
            kind,
            text: if kind == MessageKind::Prompt {
                text.to_string()
            } else {
                String::new()
            },
        });
    }

    fn line(&mut self, t: &mut Transcript, line: Line) {
        let at = line.timestamp.as_deref().and_then(parse_timestamp);
        let Some(message) = line.message.0 else {
            return;
        };
        match line.kind.as_deref() {
            Some("assistant") => self.assistant(t, at, message),
            Some("user") => self.user(t, at, message),
            _ => {}
        }
    }

    fn assistant(&mut self, t: &mut Transcript, at: Option<i64>, message: Message) {
        let id = message.id.0;
        let continues = id.is_some() && id == self.message && !t.requests.is_empty();
        if !continues {
            let start = t.calls.len();
            t.requests.push(Request {
                at,
                calls: start..start,
                id: id.as_deref().map(request_id),
                ..Default::default()
            });
            self.message = id;
        }
        let request = t.requests.last_mut().expect("a request was just pushed");
        if let Some(usage) = message.usage.0 {
            let input = [
                usage.input_tokens,
                usage.cache_creation_input_tokens,
                usage.cache_read_input_tokens,
            ]
            .iter()
            .filter_map(|n| n.u64())
            .sum::<u64>();
            // Synthetic messages (errors, interruptions) report all zeros.
            if input > 0 && request.input_tokens.is_none() {
                request.input_tokens = Some(input);
            }
            if let Some(output) = usage.output_tokens.u64() {
                request.output_tokens = Some(request.output_tokens.unwrap_or(0).max(output));
            }
        }
        let Content::Blocks(blocks) = message.content else {
            return;
        };
        for block in blocks {
            if block.kind.as_deref() != Some("tool_use") {
                continue;
            }
            let name = block.name.as_deref().unwrap_or_default();
            let mut call = Call::new(tool_kind(name));
            call.at = at;
            let input = block.input.0;
            if self.detail {
                call.line = self.line_no;
                call.name = Some(name.to_string());
                call.target = input.as_ref().and_then(Input::target);
            }
            if call.tool == Tool::Shell {
                call.command = input.and_then(|i| i.command.0);
            }
            if let Some(id) = block.id.0 {
                self.pending.insert(id, t.calls.len());
            }
            t.calls.push(call);
            request.calls.end = t.calls.len();
        }
    }

    fn user(&mut self, t: &mut Transcript, at: Option<i64>, message: Message) {
        let Content::Blocks(blocks) = message.content else {
            return;
        };
        for block in blocks {
            if block.kind.as_deref() != Some("tool_result") {
                continue;
            }
            let Some(index) = block
                .tool_use_id
                .as_deref()
                .and_then(|id| self.pending.remove(id))
            else {
                continue;
            };
            let call = &mut t.calls[index];
            if let (Some(start), Some(end)) = (call.at, at) {
                call.duration_ms = u64::try_from(end - start).ok();
            }
            call.output_bytes = block.content.bytes;
            call.rate_limited = block.content.rate_limited;
            call.failed = block.is_error.0;
            if !self.detail {
                continue;
            }
            let head = &block.content.head;
            if head.starts_with(REFUSED) {
                let reason = head.find(REASON).map_or("", |i| &head[i + REASON.len()..]);
                t.messages.push(event::Message {
                    at,
                    line: self.line_no,
                    after_calls: t.calls.len(),
                    kind: MessageKind::Refusal,
                    text: reason.trim().to_string(),
                });
            } else if call.failed {
                call.error = Some(head.clone());
            }
        }
    }
}

/// A message ID as a request identity. The hash is fixed, so the same ID
/// is the same identity in every transcript and every run.
fn request_id(message: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    message.hash(&mut h);
    h.finish()
}

/// What a Claude Code tool is, by name.
fn tool_kind(name: &str) -> Tool {
    match name {
        "Bash" | "bash" | "PowerShell" => Tool::Shell,
        "Read" | "NotebookRead" => Tool::Read,
        "Grep" | "Glob" | "LS" => Tool::Search,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => Tool::Edit,
        "BashOutput" | "TaskOutput" | "Monitor" => Tool::Poll,
        "WebFetch" | "WebSearch" => Tool::Web,
        "Task" | "Agent" | "SendMessage" | "Workflow" => Tool::Agent,
        // GitHub MCP servers, and the desktop app's pull request tools.
        _ if name.starts_with("mcp__")
            && (name.to_ascii_lowercase().contains("github") || name.contains("_pr__")) =>
        {
            Tool::Github
        }
        _ => Tool::Other,
    }
}

/// Text Claude Code writes as the user's that the user did not type: a
/// slash command's expansion, a background task's notification, a hook's or
/// command's output, a resumed session's summary.
fn harness_text(text: &str) -> bool {
    text.is_empty()
        || text.starts_with('<')
            && [
                "<command-",
                "<local-command-",
                "<task-notification",
                "<system-reminder",
                "<bash-",
                "<user-prompt-submit-hook",
                "<side-sessions-event",
                "<ci-monitor-event",
            ]
            .iter()
            .any(|tag| text.starts_with(tag))
        || text.starts_with("Caveat: ")
        || text.starts_with("This session is being continued from a previous conversation")
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type", default)]
    kind: Str,
    #[serde(rename = "isMeta", default)]
    is_meta: Flag,
    #[serde(default)]
    timestamp: Str,
    #[serde(rename = "sessionId", default)]
    session_id: Str,
    #[serde(default)]
    cwd: Str,
    #[serde(default)]
    message: Obj<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    id: Str,
    #[serde(default)]
    usage: Obj<Usage>,
    #[serde(default)]
    content: Content,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Num,
    #[serde(default)]
    cache_creation_input_tokens: Num,
    #[serde(default)]
    cache_read_input_tokens: Num,
    #[serde(default)]
    output_tokens: Num,
}

/// A message's content: plain text (a prompt), or blocks.
#[derive(Default)]
enum Content {
    #[default]
    Text,
    Blocks(Vec<Block>),
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Content;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("message content")
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Content, E> {
                Ok(Content::Text)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Content, E> {
                Ok(Content::Text)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Content, A::Error> {
                let mut blocks = Vec::new();
                while let Some(Obj(block)) = seq.next_element::<Obj<Block>>()? {
                    blocks.extend(block);
                }
                Ok(Content::Blocks(blocks))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Content, A::Error> {
                while map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(Content::Text)
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type", default)]
    kind: Str,
    #[serde(default)]
    id: Str,
    #[serde(default)]
    name: Str,
    #[serde(default)]
    input: Obj<Input>,
    #[serde(default)]
    tool_use_id: Str,
    #[serde(default)]
    content: Output,
    #[serde(default)]
    is_error: Flag,
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    command: Str,
    #[serde(default)]
    file_path: Str,
    #[serde(default)]
    notebook_path: Str,
    #[serde(default)]
    pattern: Str,
    #[serde(default)]
    path: Str,
}

impl Input {
    /// The file a read or edit names, or a search's pattern and where.
    fn target(&self) -> Option<String> {
        if let Some(file) = self.file_path.0.as_ref().or(self.notebook_path.0.as_ref()) {
            return Some(file.clone());
        }
        let pattern = self.pattern.0.as_ref()?;
        Some(match &self.path.0 {
            Some(path) => format!("{pattern} in {path}"),
            None => pattern.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str, filter: &Filter) -> Option<Transcript> {
        let source = Source {
            path: PathBuf::from("/x/projects/-p/abc.jsonl"),
            bytes: 0,
            modified_ms: None,
        };
        ClaudeCode {
            root: PathBuf::from("/x"),
        }
        .read(&source, &mut text.as_bytes(), filter)
        .unwrap()
    }

    const SESSION: &str = concat!(
        r#"{"type":"user","timestamp":"2026-10-30T10:00:00.000Z","sessionId":"s1","cwd":"/home/me/src/app","message":{"role":"user","content":"fix the build"}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-10-30T10:00:01.000Z","sessionId":"s1","cwd":"/home/me/src/app","message":{"id":"m1","usage":{"input_tokens":3,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":20},"content":[{"type":"thinking","thinking":"..."}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-10-30T10:00:02.000Z","sessionId":"s1","cwd":"/home/me/src/app","message":{"id":"m1","usage":{"input_tokens":3,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":45},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test 2>&1 | tail -5"}}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2026-10-30T10:00:32.500Z","sessionId":"s1","cwd":"/home/me/src/app","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"test result: FAILED","is_error":true}]}}"#,
        "\n",
        "{\"type\":\"assistant\",\"truncated",
        "\n",
        r#"{"type":"assistant","timestamp":"2026-10-30T10:00:40.000Z","sessionId":"s1","cwd":"/home/me/src/app","message":{"id":"m2","usage":{"input_tokens":5,"cache_read_input_tokens":1100,"output_tokens":9},"content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/a","old_string":"x","new_string":"y"}},{"type":"tool_use","id":"t3","name":"mcp__github__get_pr","input":{"command":["not","a","string"]}}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2026-10-30T10:00:41.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"ok"}]}]}}"#,
        "\n",
    );

    #[test]
    fn reads_requests_calls_tokens_and_durations() {
        let t = read(SESSION, &Filter::default()).unwrap();
        assert_eq!(t.session, "s1");
        assert_eq!(t.cwd.as_deref(), Some("/home/me/src/app"));
        assert_eq!(t.bad_lines, 1);
        assert_eq!(t.requests.len(), 2);
        assert_eq!(t.requests[0].input_tokens, Some(1003));
        assert_eq!(t.requests[0].output_tokens, Some(45));
        assert_eq!(t.requests[0].calls, 0..1);
        assert_eq!(t.requests[1].calls, 1..3);
        let bash = &t.calls[0];
        assert_eq!(bash.command.as_deref(), Some("cargo test 2>&1 | tail -5"));
        assert_eq!(bash.duration_ms, Some(30_500));
        assert!(bash.failed);
        assert_eq!(bash.output_bytes, 19);
        assert_eq!(t.calls[1].tool, Tool::Edit);
        assert_eq!(t.calls[1].output_bytes, 2);
        assert_eq!(t.calls[2].tool, Tool::Github);
        assert_eq!(t.calls[2].duration_ms, None);
    }

    #[test]
    fn filters_by_project() {
        let inside = Filter {
            project: Some(PathBuf::from("/home/me/src")),
            ..Default::default()
        };
        assert!(read(SESSION, &inside).is_some());
        let outside = Filter {
            project: Some(PathBuf::from("/home/me/src/ap")),
            ..Default::default()
        };
        assert!(read(SESSION, &outside).is_none());
    }

    #[test]
    fn subagents_belong_to_their_session() {
        let source = Source {
            path: PathBuf::from("/x/projects/-p/s9/subagents/agent-a1.jsonl"),
            bytes: 0,
            modified_ms: None,
        };
        assert_eq!(session_of(&source), "s9");
        let workflow = Source {
            path: PathBuf::from("/x/projects/-p/s9/subagents/workflows/wf_1/agent-a2.jsonl"),
            ..source
        };
        assert_eq!(session_of(&workflow), "s9");
        assert_eq!(encode("/home/me/src/x/.claude"), "-home-me-src-x--claude");
    }
}
