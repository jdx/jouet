//! What each harness sends its hooks, read as Claude Code's payload, and
//! jouet's answer, written in the harness's own fields.
//!
//! `jouet hook` works on Claude Code's shape: `session_id`, `cwd`,
//! `transcript_path`, `tool_name` (Bash, Read, Grep, Edit, Write) and
//! `tool_input` with Claude Code's field names, so every rule matches the
//! same calls whichever harness made them. Codex already sends that shape.
//! Cursor, Gemini CLI and Copilot are translated here: their tools are
//! renamed, their fields mapped, and the exit code a post-tool-use hook needs
//! is read from wherever the harness reports it. An exit code that cannot be
//! read is left out, so the command is not recorded rather than recorded
//! wrongly.

use crate::setup::Harness;
use serde_json::{Map, Value, json};

/// The harness's payload in Claude Code's shape, for the jouet hook event
/// `event`.
pub(crate) fn normalize(harness: Harness, event: &str, payload: Value) -> Value {
    match harness {
        Harness::Claude | Harness::Codex => payload,
        Harness::Cursor => cursor(event, &payload),
        Harness::Gemini => gemini(event, &payload),
        Harness::Copilot => copilot(event, &payload),
    }
}

/// A payload being built in Claude Code's shape.
#[derive(Default)]
struct Normal {
    session: Option<String>,
    cwd: Option<String>,
    transcript: Option<String>,
    tool: Option<String>,
    input: Map<String, Value>,
    /// The command's exit code, when the harness reported one.
    exit_code: Option<i64>,
    stop_hook_active: bool,
    tool_use_id: Option<String>,
    duration_ms: Option<u64>,
}

impl Normal {
    fn into_value(self, event: &str) -> Value {
        let mut out = Map::new();
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                out.insert(key.into(), value);
            }
        };
        put("session_id", self.session.map(Value::from));
        put("cwd", self.cwd.map(Value::from));
        put("transcript_path", self.transcript.map(Value::from));
        // A post-tool-use event without an exit code would read as a
        // success, so it is left unnamed: the call is then not recorded.
        let known = self.exit_code.is_some() || !event.starts_with("post-tool-use");
        let name = match event {
            "session-start" => Some("SessionStart"),
            "pre-tool-use" => Some("PreToolUse"),
            "post-tool-use" | "post-tool-use-failure" | "post-edit" => Some("PostToolUse"),
            "stop" => Some("Stop"),
            _ => None,
        };
        put("hook_event_name", name.filter(|_| known).map(Value::from));
        put("tool_name", self.tool.map(Value::from));
        put(
            "tool_input",
            Some(Value::Object(self.input))
                .filter(|i| i.as_object().is_some_and(|o| !o.is_empty())),
        );
        put("tool_use_id", self.tool_use_id.map(Value::from));
        put("exit_code", self.exit_code.map(Value::from));
        put("duration_ms", self.duration_ms.map(Value::from));
        put(
            "stop_hook_active",
            self.stop_hook_active.then_some(json!(true)),
        );
        Value::Object(out)
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Renames `from` to `to` in a tool's input, unless `to` is already there.
fn rename(input: &mut Map<String, Value>, from: &str, to: &str) {
    if !input.contains_key(to)
        && let Some(value) = input.remove(from)
    {
        input.insert(to.into(), value);
    }
}

/// Cursor: `conversation_id`, `workspace_roots`, tools named Shell, Read,
/// Grep and Write, a Shell's exit code in the JSON text of `tool_output`,
/// and `loop_count` in place of `stop_hook_active`. `afterFileEdit` names the
/// edited file outside any tool call.
fn cursor(event: &str, payload: &Value) -> Value {
    let input = payload.get("tool_input");
    let mut normal = Normal {
        session: text(payload.get("session_id")).or_else(|| text(payload.get("conversation_id"))),
        // A cursor-agent Shell call can send an empty `cwd`.
        cwd: text(payload.get("cwd"))
            .or_else(|| text(input.and_then(|i| i.get("working_directory"))))
            .or_else(|| text(input.and_then(|i| i.get("cwd"))))
            .or_else(|| text(payload.pointer("/workspace_roots/0")))
            .or_else(|| {
                std::env::var("CURSOR_PROJECT_DIR")
                    .ok()
                    .filter(|d| !d.is_empty())
            }),
        transcript: text(payload.get("transcript_path")),
        tool_use_id: text(payload.get("tool_use_id")),
        duration_ms: payload.get("duration").and_then(Value::as_u64),
        ..Normal::default()
    };
    if event == "post-edit" {
        normal.tool = Some("Edit".into());
        if let Some(file) = payload.get("file_path") {
            normal.input.insert("file_path".into(), file.clone());
        }
        return normal.into_value(event);
    }
    if event == "stop" {
        // Only a turn that completed is gated, and only the first time.
        normal.stop_hook_active = payload.get("loop_count").and_then(Value::as_u64) > Some(0)
            || payload
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "completed");
        return normal.into_value(event);
    }
    let tool = payload.get("tool_name").and_then(Value::as_str);
    normal.tool = tool.map(|t| if t == "Shell" { "Bash" } else { t }.to_string());
    normal.input = input
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in ["path", "target_file"] {
        rename(&mut normal.input, key, "file_path");
    }
    if event.starts_with("post-tool-use") {
        let output = payload.get("tool_output");
        let parsed = match output {
            Some(Value::String(s)) => serde_json::from_str(s).ok(),
            other => other.cloned(),
        };
        let code = parsed.as_ref().and_then(|o| {
            o.get("exitCode")
                .or_else(|| o.get("exit_code"))
                .and_then(Value::as_i64)
        });
        normal.exit_code = match (tool, event) {
            (Some("Shell"), _) => code,
            (_, "post-tool-use") => Some(0),
            _ => None,
        };
    }
    normal.into_value(event)
}

/// Gemini CLI: Claude Code's base fields, its own tool names, `read_file`'s
/// line range, and a shell command's exit code only in the text the model
/// sees, as an `Exit Code: N` line that is left out for 0.
fn gemini(event: &str, payload: &Value) -> Value {
    let mut normal = Normal {
        session: text(payload.get("session_id")),
        cwd: text(payload.get("cwd")),
        transcript: text(payload.get("transcript_path")),
        stop_hook_active: payload.get("stop_hook_active").and_then(Value::as_bool) == Some(true),
        ..Normal::default()
    };
    if event == "session-start" || event == "stop" {
        return normal.into_value(event);
    }
    let name = payload.get("tool_name").and_then(Value::as_str);
    let mut input = payload
        .get("tool_input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let tool = match name {
        Some("run_shell_command") => {
            // The command runs in `dir_path`, under the session's directory.
            if let (Some(cwd), Some(dir)) = (&normal.cwd, text(input.get("dir_path"))) {
                normal.cwd = Some(std::path::Path::new(cwd).join(dir).display().to_string());
            }
            rename(&mut input, "is_background", "run_in_background");
            "Bash"
        }
        Some("read_file") => {
            let line = |key: &str| input.get(key).and_then(Value::as_u64);
            if let Some(end) = line("end_line") {
                let start = line("start_line").unwrap_or(1).max(1);
                input.insert("limit".into(), json!(end.saturating_sub(start) + 1));
            }
            rename(&mut input, "start_line", "offset");
            input.remove("end_line");
            "Read"
        }
        Some("grep_search" | "search_file_content") => {
            rename(&mut input, "dir_path", "path");
            rename(&mut input, "include_pattern", "glob");
            "Grep"
        }
        Some("replace") => "Edit",
        Some("write_file") => "Write",
        Some(other) => other,
        None => "",
    };
    normal.tool = Some(tool.to_string()).filter(|t| !t.is_empty());
    normal.input = input;
    if event.starts_with("post-tool-use") {
        let response = payload.get("tool_response");
        let failed = response
            .and_then(|r| r.get("error"))
            .is_some_and(|e| !e.is_null());
        normal.exit_code = if tool == "Bash" {
            response
                .and_then(|r| r.get("llmContent"))
                .and_then(Value::as_str)
                .and_then(gemini_exit_code)
        } else if failed {
            None
        } else {
            Some(0)
        };
    }
    normal.into_value(event)
}

/// The exit code in the text Gemini CLI's shell tool gives the model:
/// `Output: ...`, then `Exit Code: N` unless it is 0. A signal, a timeout or
/// a command moved to the background has none.
fn gemini_exit_code(content: &str) -> Option<i64> {
    // The command's own output comes first, so the last such line is the
    // tool's.
    let lines: Vec<&str> = content.lines().collect();
    if let Some(code) = lines
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("Exit Code: "))
    {
        return code.trim().parse().ok();
    }
    let finished = lines.iter().any(|l| l.starts_with("Output: "))
        && !lines.iter().any(|l| l.starts_with("Signal: "));
    finished.then_some(0)
}

/// Copilot: camelCase fields (`sessionId`, `toolName`, `toolArgs` as JSON
/// text), or snake_case for hooks registered under Claude Code's event
/// names; its own tool names and fields (`view`'s `path` and `view_range`,
/// `edit`'s `old_str`); and a shell command's exit code in its result text,
/// `<exited with exit code N>`.
fn copilot(event: &str, payload: &Value) -> Value {
    let field = |camel: &str, snake: &str| payload.get(camel).or_else(|| payload.get(snake));
    let mut normal = Normal {
        session: text(field("sessionId", "session_id")),
        cwd: text(payload.get("cwd")),
        transcript: text(field("transcriptPath", "transcript_path")),
        stop_hook_active: payload.get("stop_hook_active").and_then(Value::as_bool) == Some(true),
        ..Normal::default()
    };
    if event == "session-start" || event == "stop" {
        return normal.into_value(event);
    }
    let name = field("toolName", "tool_name").and_then(Value::as_str);
    let mut input = match field("toolArgs", "tool_input") {
        Some(Value::String(args)) => serde_json::from_str(args).ok(),
        Some(Value::Object(args)) => Some(args.clone()),
        _ => None,
    }
    .unwrap_or_default();
    rename(&mut input, "path", "file_path");
    let tool = match name {
        Some("bash" | "powershell") => "Bash",
        Some("view") => {
            let range = input.remove("view_range");
            let bound = |i: usize| {
                range
                    .as_ref()
                    .and_then(|r| r.get(i))
                    .and_then(Value::as_i64)
            };
            if let Some(start) = bound(0) {
                input.insert("offset".into(), json!(start));
                if let Some(end) = bound(1).filter(|e| *e >= start) {
                    input.insert("limit".into(), json!(end - start + 1));
                }
            }
            "Read"
        }
        Some("create") => {
            rename(&mut input, "file_text", "content");
            "Write"
        }
        Some("edit" | "str_replace_editor") => {
            rename(&mut input, "old_str", "old_string");
            rename(&mut input, "new_str", "new_string");
            "Edit"
        }
        Some("grep" | "rg") => "Grep",
        Some("glob") => "Glob",
        Some(other) => other,
        None => "",
    };
    normal.tool = Some(tool.to_string()).filter(|t| !t.is_empty());
    normal.input = input;
    if event.starts_with("post-tool-use") {
        let result = payload
            .pointer("/toolResult/textResultForLlm")
            .or_else(|| payload.pointer("/tool_result/text_result_for_llm"))
            .and_then(Value::as_str);
        let error = payload.get("error").and_then(Value::as_str);
        normal.exit_code = match (tool, event) {
            ("Bash", _) => result.or(error).and_then(copilot_exit_code),
            (_, "post-tool-use") => Some(0),
            _ => None,
        };
    }
    normal.into_value(event)
}

/// The last `exit code N` in a Copilot shell result (`<exited with exit
/// code N>`) or error.
fn copilot_exit_code(text: &str) -> Option<i64> {
    const NEEDLE: &str = "exit code ";
    let at = text.rfind(NEEDLE)?;
    let rest = &text[at + NEEDLE.len()..];
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

/// What jouet decided, from its answer in Claude Code's shape.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Answer {
    /// The reason a tool call is denied.
    pub(crate) deny: Option<String>,
    /// Context for the agent: the primer, a warning, a fix after an edit.
    pub(crate) context: Option<String>,
    /// The reason a stop is blocked.
    pub(crate) block: Option<String>,
    /// A note for the user.
    pub(crate) note: Option<String>,
}

impl Answer {
    pub(crate) fn of(output: &Value) -> Self {
        let specific = output.get("hookSpecificOutput");
        let field = |v: Option<&Value>, key: &str| text(v.and_then(|v| v.get(key)));
        let denied = field(specific, "permissionDecision").as_deref() == Some("deny");
        let blocked = field(Some(output), "decision").as_deref() == Some("block");
        Self {
            deny: field(specific, "permissionDecisionReason").filter(|_| denied),
            context: field(specific, "additionalContext"),
            block: field(Some(output), "reason").filter(|_| blocked),
            note: field(Some(output), "systemMessage"),
        }
    }
}

/// jouet's answer in the harness's own fields. Cursor, Gemini CLI and
/// Copilot read stdout as JSON even with nothing to say, so they get `{}`;
/// for Claude Code and Codex, `None` prints nothing.
pub(crate) fn render(harness: Harness, event: &str, output: Option<Value>) -> Option<Value> {
    let answer = output.as_ref().map(Answer::of).unwrap_or_default();
    let mut out = Map::new();
    match harness {
        Harness::Claude | Harness::Codex => return output,
        Harness::Cursor => {
            if let Some(reason) = answer.deny {
                out.insert("permission".into(), json!("deny"));
                out.insert("user_message".into(), json!(reason));
                out.insert("agent_message".into(), json!(reason));
            }
            if let Some(context) = answer.context {
                out.insert("additional_context".into(), json!(context));
            }
            // A follow-up message is Cursor's way to keep the agent going.
            // It has no note for the user at a stop.
            if let Some(reason) = answer.block {
                out.insert("followup_message".into(), json!(reason));
            }
        }
        Harness::Gemini => {
            let reason = answer.deny.map(|r| ("deny", r));
            let reason = reason.or(answer.block.map(|r| ("block", r)));
            if let Some((decision, reason)) = reason {
                out.insert("decision".into(), json!(decision));
                out.insert("reason".into(), json!(reason));
            }
            if let Some(context) = answer.context {
                let name = match event {
                    "session-start" => "SessionStart",
                    "pre-tool-use" => "BeforeTool",
                    "stop" => "AfterAgent",
                    _ => "AfterTool",
                };
                out.insert(
                    "hookSpecificOutput".into(),
                    json!({"hookEventName": name, "additionalContext": context}),
                );
            }
            if let Some(note) = answer.note {
                out.insert("systemMessage".into(), json!(note));
            }
        }
        Harness::Copilot => {
            if let Some(reason) = answer.deny {
                out.insert("permissionDecision".into(), json!("deny"));
                out.insert("permissionDecisionReason".into(), json!(reason));
            }
            if let Some(context) = answer.context {
                out.insert("additionalContext".into(), json!(context));
            }
            if let Some(reason) = answer.block {
                out.insert("decision".into(), json!("block"));
                out.insert("reason".into(), json!(reason));
            }
        }
    }
    Some(Value::Object(out))
}

/// Whether the harness has no way to tell the agent something before a
/// tool runs without denying it, so a rule's warning is held for the
/// post-tool-use hook.
pub(crate) fn defers_warnings(harness: Harness) -> bool {
    matches!(harness, Harness::Cursor | Harness::Gemini)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_reports_a_nonzero_exit_and_leaves_out_zero() {
        let wrapped = |body: &str| format!("<untrusted_context>\n{body}\n</untrusted_context>");
        assert_eq!(
            gemini_exit_code(&wrapped("Output: ok\nProcess Group PGID: 7")),
            Some(0)
        );
        assert_eq!(
            gemini_exit_code(&wrapped(
                "Output: Exit Code: 3\nfail\nExit Code: 101\nProcess Group PGID: 7"
            )),
            Some(101)
        );
        assert_eq!(
            gemini_exit_code(&wrapped(
                "Output: (empty)\nSignal: 9\nProcess Group PGID: 7"
            )),
            None
        );
        assert_eq!(
            gemini_exit_code("Command moved to background (PID: 4). Output hidden."),
            None
        );
    }

    #[test]
    fn copilot_reports_the_last_exit_code() {
        assert_eq!(copilot_exit_code("ok\n<exited with exit code 0>"), Some(0));
        assert_eq!(
            copilot_exit_code("exited with exit code 0\n<exited with exit code 2>"),
            Some(2)
        );
        assert_eq!(copilot_exit_code("no marker"), None);
    }

    #[test]
    fn an_unknown_exit_code_leaves_the_event_unnamed() {
        let out = copilot(
            "post-tool-use",
            &json!({"toolName": "bash", "toolArgs": "{\"command\":\"cargo test\"}", "toolResult": {"resultType": "success", "textResultForLlm": "ok"}}),
        );
        assert_eq!(out.get("hook_event_name"), None, "{out}");
        assert_eq!(out.get("exit_code"), None, "{out}");
        assert_eq!(out["tool_input"]["command"], "cargo test");
    }

    #[test]
    fn answers_are_written_in_each_harness_fields() {
        let deny = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "no"}});
        assert_eq!(
            render(Harness::Cursor, "pre-tool-use", Some(deny.clone())),
            Some(json!({"permission": "deny", "user_message": "no", "agent_message": "no"}))
        );
        assert_eq!(
            render(Harness::Gemini, "pre-tool-use", Some(deny.clone())),
            Some(json!({"decision": "deny", "reason": "no"}))
        );
        assert_eq!(
            render(Harness::Copilot, "pre-tool-use", Some(deny.clone())),
            Some(json!({"permissionDecision": "deny", "permissionDecisionReason": "no"}))
        );
        assert_eq!(
            render(Harness::Claude, "pre-tool-use", Some(deny.clone())),
            Some(deny)
        );
        let block = json!({"decision": "block", "reason": "tests failed"});
        assert_eq!(
            render(Harness::Cursor, "stop", Some(block)),
            Some(json!({"followup_message": "tests failed"}))
        );
        assert_eq!(render(Harness::Gemini, "stop", None), Some(json!({})));
        assert_eq!(render(Harness::Codex, "stop", None), None);
    }
}
