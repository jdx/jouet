//! Codex rollouts: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and
//! `~/.codex/archived_sessions/rollout-*.jsonl.zst`.
//!
//! A `token_count` event follows each request's response and tool runs, so
//! the calls issued since the previous `token_count` belong to the request
//! it reports. Most commands run inside `exec` cells, JavaScript that calls
//! `tools.exec_command({...})`, often with bare keys, so commands are pulled
//! out of the script text; a `CommandExecution` item, when one follows,
//! supplies the exact exit code and duration.

use super::json::{LastStr, Num, Obj, Output, Str};
use super::{Filter, Reader, Source, files_under, home, lines, sources};
use crate::event::{Call, Request, Tool, Transcript};
use crate::time::parse_timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::PathBuf;

/// Reads Codex's rollouts under its home directory.
#[derive(Debug, Clone)]
pub struct Codex {
    /// `$CODEX_HOME`, or `~/.codex`.
    pub root: PathBuf,
}

impl Codex {
    pub fn from_env() -> Option<Self> {
        let root = std::env::var_os("CODEX_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".codex")))?;
        Some(Self { root })
    }
}

impl Reader for Codex {
    fn harness(&self) -> &'static str {
        "codex"
    }

    fn discover(&self, _: &Filter) -> io::Result<Vec<Source>> {
        let mut paths = Vec::new();
        for dir in ["sessions", "archived_sessions"] {
            files_under(&self.root.join(dir), &[".jsonl", ".jsonl.zst"], &mut paths)?;
        }
        Ok(sources(paths))
    }

    fn read(
        &self,
        source: &Source,
        input: &mut dyn BufRead,
        filter: &Filter,
    ) -> io::Result<Option<Transcript>> {
        let mut t = Transcript {
            session: source
                .path
                .file_name()
                .map(|n| {
                    n.to_string_lossy()
                        .trim_end_matches(".zst")
                        .trim_end_matches(".jsonl")
                        .to_string()
                })
                .unwrap_or_default(),
            ..Default::default()
        };
        let mut state = State::default();
        let mut other_project = false;
        t.truncated = lines(input, |line| {
            if !RELEVANT
                .iter()
                .any(|k| memchr::memmem::find(line, k).is_some())
            {
                return true;
            }
            let parsed: Line = match serde_json::from_slice(line) {
                Ok(parsed) => parsed,
                Err(_) => {
                    t.bad_lines += 1;
                    return true;
                }
            };
            let at = parsed.timestamp.as_deref().and_then(parse_timestamp);
            let Some(payload) = parsed.payload.0 else {
                return true;
            };
            match (parsed.kind.as_deref(), payload.kind.as_deref()) {
                (Some("session_meta"), _) => {
                    if let Some(id) = payload.id.0 {
                        t.session = id;
                    }
                    if let Some(cwd) = payload.cwd.0 {
                        t.cwd = Some(cwd);
                    }
                    if !filter.matches_cwd(t.cwd.as_deref()) {
                        other_project = true;
                        return false;
                    }
                }
                (Some("turn_context"), _) if t.cwd.is_none() => {
                    t.cwd = payload.cwd.0;
                }
                (_, Some("custom_tool_call" | "function_call")) => state.call(&mut t, at, payload),
                (_, Some("custom_tool_call_output" | "function_call_output")) => {
                    state.output(&mut t, payload);
                }
                (_, Some("token_count")) => state.token_count(&mut t, at, payload),
                (_, Some("item_completed")) => state.item(&mut t, payload),
                _ => {}
            }
            true
        });
        if other_project || !filter.matches_cwd(t.cwd.as_deref()) {
            return Ok(None);
        }
        let start = state.request_start;
        if start < t.calls.len() {
            // Calls after the last token count: their request's tokens were
            // never reported.
            let at = t.calls[start].at;
            t.requests.push(Request {
                at,
                calls: start..t.calls.len(),
                ..Default::default()
            });
        }
        Ok(Some(t))
    }
}

/// Substrings of the lines worth parsing; the rest (messages, reasoning,
/// file changes) are skipped unread.
const RELEVANT: [&[u8]; 6] = [
    b"\"session_meta\"",
    b"\"turn_context\"",
    b"tool_call",
    b"function_call",
    b"\"token_count\"",
    b"\"CommandExecution\"",
];

#[derive(Default)]
struct State {
    /// The first call not yet given to a request.
    request_start: usize,
    /// The last `total_token_usage`, to skip repeated token counts.
    last_total: Option<u64>,
    /// Calls waiting for their output, by call ID.
    pending: HashMap<String, Vec<usize>>,
    /// Shell calls waiting for an exact exit code, by command.
    commands: HashMap<String, usize>,
    /// Calls whose exit code and duration came from a `CommandExecution`.
    exact: Vec<bool>,
}

impl State {
    fn call(&mut self, t: &mut Transcript, at: Option<i64>, p: Payload) {
        let name = p.name.as_deref().unwrap_or_default();
        let src = p.input.0.or(p.arguments.0).unwrap_or_default();
        let mut calls = calls_for(name, &src);
        let start = t.calls.len();
        for call in &mut calls {
            call.at = at;
        }
        t.calls.extend(calls);
        self.exact.resize(t.calls.len(), false);
        for (i, call) in t.calls.iter().enumerate().skip(start) {
            if let Some(command) = &call.command {
                self.commands.insert(command.clone(), i);
            }
        }
        if let Some(id) = p.call_id.0 {
            self.pending.insert(id, (start..t.calls.len()).collect());
        }
    }

    fn output(&mut self, t: &mut Transcript, p: Payload) {
        let Some(indexes) = p.call_id.as_deref().and_then(|id| self.pending.remove(id)) else {
            return;
        };
        let out = p.output;
        let n = indexes.len().max(1) as u64;
        let wall = wall_time_ms(&out.head);
        let exit = exit_code(&out.head);
        let shells = indexes
            .iter()
            .filter(|&&i| t.calls[i].tool == Tool::Shell)
            .count()
            .max(1) as u64;
        // A script stops at the first command that throws: the last one it ran.
        let script_failed = out.head.starts_with("Script failed");
        let last_shell = indexes
            .iter()
            .rev()
            .find(|&&i| t.calls[i].tool == Tool::Shell)
            .copied();
        for &i in &indexes {
            let call = &mut t.calls[i];
            call.output_bytes = out.bytes / n;
            call.rate_limited = out.rate_limited;
            if let Some(command) = &call.command {
                self.commands.remove(command);
            }
            if self.exact[i] {
                continue;
            }
            // A poll reports the exit of the process it checked on, not its own.
            if call.tool == Tool::Shell {
                call.duration_ms = wall.map(|w| w / shells);
                call.failed =
                    script_failed && last_shell == Some(i) || exit.is_some_and(|c| c != 0);
                // One exit code in the output belongs to the cell's only command.
                if shells == 1 {
                    call.exit_code = exit.and_then(|c| i32::try_from(c).ok());
                }
            } else {
                call.duration_ms = wall.map(|w| w / n);
            }
        }
    }

    fn item(&mut self, t: &mut Transcript, p: Payload) {
        let Some(item) = p.item.0 else {
            return;
        };
        if item.kind.as_deref() != Some("CommandExecution") {
            return;
        }
        let Some(i) = item
            .command
            .0
            .as_deref()
            .and_then(|c| self.commands.remove(c))
        else {
            return;
        };
        let call = &mut t.calls[i];
        if let Some(code) = item.exit_code.0 {
            call.failed = code != 0.0;
            call.exit_code = Some(code as i32);
        }
        if let Some(d) = item.duration.0 {
            call.duration_ms =
                Some(d.secs.u64().unwrap_or(0) * 1000 + d.nanos.u64().unwrap_or(0) / 1_000_000);
        }
        self.exact[i] = true;
    }

    fn token_count(&mut self, t: &mut Transcript, at: Option<i64>, p: Payload) {
        let Some(info) = p.info.0 else {
            return;
        };
        let total = info.total_token_usage.0.and_then(|u| u.total_tokens.u64());
        if total.is_some() && total == self.last_total {
            return;
        }
        self.last_total = total;
        let Some(last) = info.last_token_usage.0 else {
            return;
        };
        t.requests.push(Request {
            at,
            input_tokens: last.input_tokens.u64(),
            output_tokens: last.output_tokens.u64(),
            calls: self.request_start..t.calls.len(),
            // A token count carries no ID to recognise a copy by.
            id: None,
        });
        self.request_start = t.calls.len();
    }
}

/// The calls one Codex tool call makes.
fn calls_for(name: &str, src: &str) -> Vec<Call> {
    match name {
        "exec" => exec_cell(src),
        "exec_command" | "shell" | "shell_command" | "local_shell" => {
            vec![shell_args(src).map_or_else(|| Call::new(Tool::Shell), Call::shell)]
        }
        _ => vec![Call::new(tool_kind(name))],
    }
}

/// What a Codex tool is, by name.
fn tool_kind(name: &str) -> Tool {
    match name {
        "apply_patch" => Tool::Edit,
        "wait" | "write_stdin" | "sleep" => Tool::Poll,
        "spawn_agent" | "send_message" | "wait_agent" | "followup_task" | "close_agent" => {
            Tool::Agent
        }
        "web__run" | "web_search" => Tool::Web,
        "view_image" => Tool::Read,
        // The GitHub connector's tools: `_fetch_pr`, `_get_pr_info`, ...
        _ if name.starts_with('_') || name.to_ascii_lowercase().contains("github") => Tool::Github,
        _ => Tool::Other,
    }
}

/// The command in a shell tool's JSON arguments: `{"cmd": "..."}` or
/// `{"command": ["bash", "-lc", "..."]}`.
fn shell_args(src: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Args {
        #[serde(default)]
        cmd: Str,
        #[serde(default)]
        command: LastStr,
    }
    let args: Args = serde_json::from_str(src).ok()?;
    args.cmd.0.or(args.command.0)
}

/// The calls an `exec` cell's script makes, in the order they are written.
/// A script that calls no known tool is one [`Tool::Other`] call.
pub(crate) fn exec_cell(src: &str) -> Vec<Call> {
    let mut calls = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("tools.") {
        let name_start = from + at + "tools.".len();
        let name_len = src[name_start..]
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
            .count();
        let name = &src[name_start..name_start + name_len];
        from = name_start + name_len;
        if !src[from..].starts_with('(') {
            continue;
        }
        let args = &src[from + 1..];
        let tool = match name {
            "exec_command" => {
                calls.push(js_cmd(args).map_or_else(|| Call::new(Tool::Shell), Call::shell));
                continue;
            }
            "write_stdin" | "wait" => Tool::Poll,
            "apply_patch" => Tool::Edit,
            "web__run" => Tool::Web,
            "view_image" => Tool::Read,
            "update_plan" | "request_user_input" => Tool::Other,
            "spawn_agent" | "send_message" | "wait_agent" => Tool::Agent,
            _ if name.starts_with("mcp__") || name.starts_with("codex_app__") => tool_kind(name),
            // Methods on other objects named `tools`, or helpers: not calls.
            _ => continue,
        };
        calls.push(Call::new(tool));
    }
    if calls.is_empty() {
        calls.push(Call::new(Tool::Other));
    }
    calls
}

/// The `cmd` of an `exec_command` argument object: JSON, or a JavaScript
/// object literal with bare keys and `'`, `"` or `` ` `` strings.
fn js_cmd(args: &str) -> Option<String> {
    let mut stream = serde_json::Deserializer::from_str(args).into_iter::<serde_json::Value>();
    if let Some(Ok(value)) = stream.next() {
        return value
            .get("cmd")
            .and_then(|c| c.as_str())
            .map(str::to_string);
    }
    // Look for `cmd:` close to the opening brace.
    let window = &args[..floor_char_boundary(args, 200)];
    let mut search = 0;
    let key_end = loop {
        let at = window[search..].find("cmd")? + search;
        search = at + 3;
        let before = window[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = window[search..]
            .trim_start_matches(['"', '\''])
            .trim_start();
        if let Some(after) = rest.strip_prefix(':') {
            // `after` ends where `window` does, so this is its offset in `args`.
            break window.len() - after.len();
        }
    };
    let value = args[key_end..].trim_start();
    let quote = value.chars().next()?;
    match quote {
        '"' => serde_json::Deserializer::from_str(value)
            .into_iter::<String>()
            .next()?
            .ok(),
        '\'' | '`' => {
            let mut out = String::new();
            let mut chars = value[1..].chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next()? {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        other => out.push(other),
                    },
                    c if c == quote => return Some(out),
                    c => out.push(c),
                }
            }
            None
        }
        _ => None,
    }
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// `Wall time: 2.0 seconds` or `Wall time 11.0 seconds`, in milliseconds.
fn wall_time_ms(head: &str) -> Option<u64> {
    let rest = &head[head.find("Wall time")? + "Wall time".len()..];
    let rest = rest.trim_start_matches(':').trim_start();
    let number: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let secs: f64 = number.parse().ok()?;
    Some((secs * 1000.0).round() as u64)
}

/// `Process exited with code N` or `Exit code: N`.
fn exit_code(head: &str) -> Option<i64> {
    ["Process exited with code ", "Exit code: "]
        .iter()
        .find_map(|marker| {
            let rest = &head[head.find(marker)? + marker.len()..];
            let digits: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            digits.parse().ok()
        })
}

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    timestamp: Str,
    #[serde(rename = "type", default)]
    kind: Str,
    #[serde(default)]
    payload: Obj<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type", default)]
    kind: Str,
    #[serde(default)]
    id: Str,
    #[serde(default)]
    cwd: Str,
    #[serde(default)]
    name: Str,
    #[serde(default)]
    input: Str,
    #[serde(default)]
    arguments: Str,
    #[serde(default)]
    call_id: Str,
    #[serde(default)]
    output: Output,
    #[serde(default)]
    info: Obj<Info>,
    #[serde(default)]
    item: Obj<Item>,
}

#[derive(Deserialize)]
struct Info {
    #[serde(default)]
    last_token_usage: Obj<Usage>,
    #[serde(default)]
    total_token_usage: Obj<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Num,
    #[serde(default)]
    output_tokens: Num,
    #[serde(default)]
    total_tokens: Num,
}

#[derive(Deserialize)]
struct Item {
    #[serde(rename = "type", default)]
    kind: Str,
    #[serde(default)]
    command: LastStr,
    #[serde(default)]
    exit_code: Num,
    #[serde(default)]
    duration: Obj<Duration>,
}

#[derive(Deserialize)]
struct Duration {
    #[serde(default)]
    secs: Num,
    #[serde(default)]
    nanos: Num,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(src: &str) -> Vec<Option<String>> {
        exec_cell(src).into_iter().map(|c| c.command).collect()
    }

    #[test]
    fn pulls_commands_out_of_exec_cells() {
        assert_eq!(
            commands(
                r#"const r = await tools.exec_command({"cmd":"gh pr checks 1","yield_time_ms":1000}); text(r)"#
            ),
            [Some("gh pr checks 1".to_string())]
        );
        assert_eq!(
            commands("await tools.exec_command({cmd: 'git status --short', workdir: '/x'})"),
            [Some("git status --short".to_string())]
        );
        assert_eq!(
            commands(
                "await tools.exec_command({ cmd: `cargo test -p ${crate}\\nexit`, max_output_tokens: 4000 })"
            ),
            [Some("cargo test -p ${crate}\nexit".to_string())]
        );
        assert_eq!(
            commands("await tools.exec_command({cmd: \"echo \\\"hi\\\"\"})"),
            [Some("echo \"hi\"".to_string())]
        );
        // A command held in a variable cannot be read.
        assert_eq!(commands("await tools.exec_command({cmd: c})"), [None]);
    }

    #[test]
    fn exec_cells_poll_edit_and_compute() {
        let kinds = |src: &str| {
            exec_cell(src)
                .into_iter()
                .map(|c| c.tool)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(
                "await tools.write_stdin({session_id: 3, chars: ''}); await tools.apply_patch(`*** Begin Patch`)"
            ),
            [Tool::Poll, Tool::Edit]
        );
        assert_eq!(
            kinds("const x = tools.length; [1,2].map(n => n * 2)"),
            [Tool::Other]
        );
        assert_eq!(
            kinds("await tools.mcp__codex_apps__github_fetch_pr({})"),
            [Tool::Github]
        );
    }

    #[test]
    fn reads_wall_times_and_exit_codes() {
        assert_eq!(
            wall_time_ms("Script completed\nWall time 0.1 seconds\nOutput:"),
            Some(100)
        );
        assert_eq!(
            wall_time_ms("Chunk ID: x\nWall time: 2.0002 seconds"),
            Some(2000)
        );
        assert_eq!(
            exit_code("Wall time: 2 seconds\nProcess exited with code 101\n"),
            Some(101)
        );
        assert_eq!(exit_code("Exit code: 0\n"), Some(0));
        assert_eq!(exit_code("Output:\nfine"), None);
    }
}
