//! What a reader produces and an analyzer consumes.

use std::ops::Range;

/// One transcript: a session's main conversation, or one of its subagents.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    /// The session this transcript belongs to. A subagent's transcript
    /// carries its parent's ID, so sessions are counted once.
    pub session: String,
    /// The directory the session ran in, when the transcript says.
    pub cwd: Option<String>,
    /// Every tool call, in the order the requests issued them.
    pub calls: Vec<Call>,
    /// Every model request, in order.
    pub requests: Vec<Request>,
    /// Lines that were not valid JSON, such as the last line of a file that
    /// was still being written. They are skipped.
    pub bad_lines: u64,
    /// Reading stopped early with this error; what came before it is kept.
    pub truncated: Option<String>,
    /// What the user said, and when they interrupted or refused a call. Read
    /// only with [`Filter::detail`](crate::Filter::detail).
    pub messages: Vec<Message>,
    /// Lines in the file, counting those skipped.
    pub lines: u64,
    /// What the harness sent with every request besides the conversation,
    /// as the transcript first recorded it. Only Claude Code records it.
    pub definitions: Option<Definitions>,
}

/// The definitions a harness sends with every request: tools, the skill
/// listing and MCP servers' instructions. Claude Code records a snapshot of
/// them when a session starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Definitions {
    /// When the snapshot was taken, in milliseconds since the Unix epoch.
    pub at: Option<i64>,
    pub items: Vec<Definition>,
}

/// One definition, and its size as sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub kind: DefinitionKind,
    /// A tool's name (`Bash`, `mcp__server__tool`), a skill's
    /// (`plugin:skill`), or an MCP server's.
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefinitionKind {
    /// A tool's JSON definition.
    Tool,
    /// A skill's line in the skill listing.
    Skill,
    /// An MCP server's instructions.
    McpInstructions,
}

/// Something the user did, as the transcript records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// When, in milliseconds since the Unix epoch.
    pub at: Option<i64>,
    /// The transcript line it is on, from 1.
    pub line: u64,
    /// How many calls the transcript had issued before it, so a message
    /// can be placed among them.
    pub after_calls: usize,
    pub kind: MessageKind,
    /// What the user wrote: a prompt's text, or the reason given with a
    /// refusal. Raw, and only its first 512 bytes, as much as the readers
    /// keep of any output; whoever shows it redacts it first.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// A prompt the user typed.
    Prompt,
    /// The user stopped the agent mid-turn.
    Interrupt,
    /// The user refused a tool call the agent asked to make.
    Refusal,
}

/// One request to the model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    /// When it was made, in milliseconds since the Unix epoch.
    pub at: Option<i64>,
    /// The context it read: every input token, cached or not, since a
    /// request is billed and timed by the whole context it re-reads.
    pub input_tokens: Option<u64>,
    /// Whether input_tokens includes every input-usage component the reader needs.
    pub input_tokens_complete: bool,
    /// Input tokens served from the prompt cache, when reported.
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// The calls this request issued, as indexes into [`Transcript::calls`].
    pub calls: Range<usize>,
    /// What identifies this request wherever it is copied: a forked or
    /// resumed Claude Code session repeats its parent's requests, message
    /// IDs and all, so [`scan`](crate::scan) counts each ID once.
    pub id: Option<u64>,
}

/// One tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// When it was issued, in milliseconds since the Unix epoch.
    pub at: Option<i64>,
    pub tool: Tool,
    /// The shell command, for [`Tool::Shell`].
    pub command: Option<String>,
    /// How long it ran.
    pub duration_ms: Option<u64>,
    /// How much output it returned to the model, in bytes.
    pub output_bytes: u64,
    /// It failed: an error result or a non-zero exit.
    pub failed: bool,
    /// Its exit code, when the source recorded one.
    pub exit_code: Option<i32>,
    /// The exit code came from a command-specific event, not a tool envelope.
    pub exit_code_exact: bool,
    /// Its output reported a GitHub rate limit.
    pub rate_limited: bool,
    /// The transcript line that issued it, from 1. Read only with
    /// [`Filter::detail`](crate::Filter::detail), like the fields below.
    pub line: u64,
    /// The tool's own name, such as `TodoWrite` or `apply_patch`. Claude
    /// Code's reader records it always.
    pub name: Option<String>,
    /// The skill a Claude Code `Skill` call loaded.
    pub skill: Option<String>,
    /// The file or pattern a read, search or edit names, raw.
    pub target: Option<String>,
    /// The start of a failed call's output, raw.
    pub error: Option<String>,
    /// The start of the output of a command that runs jouet, raw, so a
    /// digest can check jouet's own `--json`.
    pub output: Option<String>,
    /// A process check's transcript-local identity and whether it was a
    /// non-interactive check. Never included in reports.
    pub wait: Option<Wait>,
    /// A user-facing message occurred since the preceding call.
    pub wait_boundary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wait {
    pub id: String,
    /// The call did not send input and asked for a short check.
    pub short: bool,
    /// The result explicitly said the process was still running.
    pub running: bool,
    /// No process output was returned with the result.
    pub quiet: bool,
}

impl Call {
    pub fn new(tool: Tool) -> Self {
        Self {
            at: None,
            tool,
            command: None,
            duration_ms: None,
            output_bytes: 0,
            failed: false,
            exit_code: None,
            exit_code_exact: false,
            rate_limited: false,
            line: 0,
            name: None,
            skill: None,
            target: None,
            error: None,
            output: None,
            wait: None,
            wait_boundary: false,
        }
    }

    pub fn shell(command: impl Into<String>) -> Self {
        Self {
            command: Some(command.into()),
            ..Self::new(Tool::Shell)
        }
    }
}

/// What kind of tool a call used, whatever the harness names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    /// A shell command. Its category comes from the command.
    Shell,
    /// Reading a file.
    Read,
    /// Searching or listing files.
    Search,
    /// Editing or writing a file.
    Edit,
    /// Checking on a running process: Codex's `wait` and `write_stdin`,
    /// Claude Code's `BashOutput` and `TaskOutput`.
    Poll,
    /// A GitHub tool other than the shell, such as an MCP server's.
    Github,
    Web,
    /// Starting, messaging or waiting for another agent.
    Agent,
    Other,
}
