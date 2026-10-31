//! A session digest: one transcript, read in [detail](crate::Filter::detail),
//! reduced to what a reviewer needs to find improvements, in a few thousand
//! tokens instead of the session's millions.
//!
//! It keeps a timeline of phases (one per prompt) with the calls collapsed
//! by shape, the user's corrections, failures and their retries, long
//! waits, where tokens and time went, files read again and again, and
//! commands typed out more than once. Every item points back to the
//! transcript lines it came from, so a reviewer can read the original.
//!
//! Everything shown passes through [`Options::redact`] first, after the
//! home directory is written as `~`. Nothing here writes a file or touches
//! the network.

pub mod correction;
pub mod jouet;
mod render;

pub use correction::Kind;
pub use render::{duration, render, render_all};

use crate::analyze::Analyzer;
use crate::event::{Call, MessageKind, Tool, Transcript};
use crate::shape::shape;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// How a digest is built.
pub struct Options<'a> {
    /// The home directory, written as `~`.
    pub home: Option<&'a str>,
    /// Removes secrets from text before it is kept: commands, prompts,
    /// error output, file names.
    pub redact: &'a dyn Fn(&str) -> String,
}

/// Lines of a transcript, from 1, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub from: u64,
    pub to: u64,
}

impl Span {
    fn at(line: u64) -> Self {
        Self {
            from: line,
            to: line,
        }
    }

    fn join(self, other: Self) -> Self {
        Self {
            from: self.from.min(other.from),
            to: self.to.max(other.to),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub prompts: u64,
    pub requests: u64,
    pub calls: u64,
    pub failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Time spent in tool calls.
    pub tool_ms: u64,
    /// From the first line with a time to the last.
    pub wall_ms: u64,
}

/// One session, digested.
#[derive(Debug, Clone, Serialize)]
pub struct Digest {
    pub harness: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The transcript, which each item's span points into.
    pub transcript: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    pub totals: Totals,
    pub phases: Vec<Phase>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<Correction>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Failure>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waits: Vec<Wait>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sinks: Vec<Sink>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rereads: Vec<Reread>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retyped: Vec<Retyped>,
    /// Costly patterns `jouet audit` knows, with its advice.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<KnownPattern>,
    /// Where jouet itself crashed, misbehaved or fell short.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jouet: Vec<JouetFinding>,
    /// Every file read, and how often, for [`across`].
    #[serde(skip)]
    reads: BTreeMap<String, u64>,
    /// Every command typed out at length, by its text, for [`across`].
    #[serde(skip)]
    commands: BTreeMap<String, u64>,
}

/// What happened between one prompt and the next.
#[derive(Debug, Clone, Serialize)]
pub struct Phase {
    pub span: Span,
    /// Offset from the session's start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The prompt that started it, cut short; `None` before the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub requests: u64,
    pub input_tokens: u64,
    pub calls: Vec<Group>,
}

/// Calls of one kind in a phase: a tool and its targets, or a command
/// shape and an example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    pub what: String,
    pub count: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub failed: u64,
    /// Up to three distinct targets or examples.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// Distinct ones left out of `details`.
    #[serde(skip_serializing_if = "is_zero")]
    pub more: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct Correction {
    pub span: Span,
    pub kind: Kind,
    /// What the user wrote, cut short; empty for an interruption.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<&'static str>,
    /// The call the agent made last before it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// It follows an interruption: whatever it says redirects the agent.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub after_interrupt: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub span: Span,
    pub what: String,
    pub attempts: u64,
    pub failed: u64,
    /// A later attempt of the same shape succeeded.
    pub recovered: bool,
    /// The line of the first failure's output that says what went wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A read or a search, which fails when it finds nothing.
    #[serde(skip)]
    searched: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Wait {
    pub span: Span,
    pub what: String,
    pub duration_ms: u64,
}

/// A category of calls and the tokens and time `jouet audit` attributes
/// to it.
#[derive(Debug, Clone, Serialize)]
pub struct Sink {
    pub category: String,
    pub input_tokens: u64,
    pub share: f64,
    pub calls: u64,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_shape: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reread {
    pub file: String,
    pub reads: u64,
    /// Reads with no edit to the file since the one before.
    pub unchanged: u64,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Retyped {
    pub command: String,
    pub runs: u64,
    pub span: Span,
}

/// Something jouet did wrong or fell short in, found by [`jouet::scan`].
#[derive(Debug, Clone, Serialize)]
pub struct JouetFinding {
    pub span: Span,
    #[serde(flatten)]
    pub found: jouet::Found,
    /// The command, raw: whoever keeps it reduces it to a shape first.
    #[serde(skip)]
    pub command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnownPattern {
    pub pattern: String,
    pub calls: u64,
    pub advice: &'static str,
}

/// Calls at least this long are waits.
const LONG_CALL_MS: u64 = 30_000;
/// A file read this often, or re-read unchanged this often, is listed.
const REREADS: u64 = 3;
const UNCHANGED_REREADS: u64 = 2;
/// A command this long typed out twice, or any command three times.
const LONG_COMMAND: usize = 80;
const SHORT_COMMAND: usize = 24;
/// How much of a prompt, a correction or an error is kept.
const EXCERPT: usize = 160;

impl Digest {
    /// Every file the session read, relative to its directory or with the
    /// home directory as `~`.
    pub fn files_read(&self) -> impl Iterator<Item = &str> {
        self.reads.keys().map(String::as_str)
    }
}

/// Digests one transcript read with [`Filter::detail`](crate::Filter::detail).
pub fn digest(harness: &str, transcript: &Transcript, path: &str, options: &Options<'_>) -> Digest {
    let t = transcript;
    let home_to_tilde = |text: &str| match options.home {
        Some(home) if home.len() > 1 => text.replace(home, "~"),
        _ => text.to_string(),
    };
    // The session's own directory is no secret, however key-like a
    // worktree's name looks: it is kept whole, and the rest redacted.
    let cwd = t.cwd.as_deref().map(home_to_tilde).filter(|c| c.len() > 1);
    let clean = |text: &str| -> String {
        let text = home_to_tilde(text);
        match &cwd {
            Some(cwd) => text
                .split(cwd.as_str())
                .map(|piece| (options.redact)(piece))
                .collect::<Vec<_>>()
                .join(cwd),
            None => (options.redact)(&text),
        }
    };
    let start = first_time(t);
    let call_name = |call: &Call| -> String { group_key(call, t.cwd.as_deref(), &clean) };

    let totals = Totals {
        prompts: t
            .messages
            .iter()
            .filter(|m| m.kind == MessageKind::Prompt)
            .count() as u64,
        requests: t.requests.len() as u64,
        calls: t.calls.len() as u64,
        failed: t.calls.iter().filter(|c| c.failed).count() as u64,
        input_tokens: t.requests.iter().filter_map(|r| r.input_tokens).sum(),
        output_tokens: t.requests.iter().filter_map(|r| r.output_tokens).sum(),
        tool_ms: t.calls.iter().filter_map(|c| c.duration_ms).sum(),
        wall_ms: match (start, last_time(t)) {
            (Some(a), Some(b)) => u64::try_from(b - a).unwrap_or(0),
            _ => 0,
        },
    };

    let mut d = Digest {
        harness: harness.to_string(),
        session: t.session.clone(),
        project: cwd.clone(),
        // Not redacted: the spans point into it, and a long session ID in
        // its name reads as a key.
        transcript: match options.home {
            Some(home) if home.len() > 1 => path.replace(home, "~"),
            _ => path.to_string(),
        },
        started_ms: start,
        totals,
        phases: phases(t, start, &clean, &call_name),
        corrections: corrections(t, &clean, &call_name),
        failures: failures(t, &clean, &call_name),
        waits: waits(t, &call_name),
        sinks: Vec::new(),
        rereads: Vec::new(),
        retyped: Vec::new(),
        patterns: Vec::new(),
        jouet: jouet_findings(t),
        reads: BTreeMap::new(),
        commands: BTreeMap::new(),
    };
    rereads(t, &clean, &mut d);
    retyped(t, options.home, &clean, &mut d);
    attribution(harness, t, &mut d);
    d
}

fn first_time(t: &Transcript) -> Option<i64> {
    let calls = t.calls.iter().filter_map(|c| c.at);
    let messages = t.messages.iter().filter_map(|m| m.at);
    let requests = t.requests.iter().filter_map(|r| r.at);
    calls.chain(messages).chain(requests).min()
}

fn last_time(t: &Transcript) -> Option<i64> {
    let calls = t.calls.iter().filter_map(|c| {
        c.at.map(|at| {
            at + c
                .duration_ms
                .and_then(|d| i64::try_from(d).ok())
                .unwrap_or(0)
        })
    });
    let messages = t.messages.iter().filter_map(|m| m.at);
    let requests = t.requests.iter().filter_map(|r| r.at);
    calls.chain(messages).chain(requests).max()
}

/// A call's name in the digest: a command's shape, or the tool, with the
/// file for an edit.
fn group_key(call: &Call, cwd: Option<&str>, clean: &dyn Fn(&str) -> String) -> String {
    let key = match (&call.command, call.tool) {
        (Some(command), _) => shape(command),
        (None, tool) => tool_label(call, tool),
    };
    match (call.tool, call.target.as_deref()) {
        (Tool::Edit, Some(target)) => format!("{key} {}", relative(target, cwd, clean)),
        _ => key,
    }
}

/// A tool call's name: the harness's own, when it was read.
fn tool_label(call: &Call, tool: Tool) -> String {
    match (&call.name, tool) {
        (Some(name), Tool::Other | Tool::Agent | Tool::Github | Tool::Web | Tool::Poll) => {
            name.clone()
        }
        _ => tool_name(tool).to_string(),
    }
}

/// Programs whose first word or two after the program say what it does:
/// `gh pr view`, `git log`, `mise run test`.
const SUBCOMMANDS: &[(&str, usize)] = &[
    ("gh", 2),
    ("git", 1),
    ("cargo", 1),
    ("mise", 2),
    ("npm", 2),
    ("pnpm", 2),
    ("yarn", 1),
    ("bun", 2),
    ("docker", 2),
    ("kubectl", 1),
    ("jouet", 2),
    ("go", 1),
    ("rustup", 1),
    ("uv", 1),
    ("pip", 1),
    ("brew", 1),
    ("hk", 1),
    ("make", 1),
    ("just", 1),
    ("systemctl", 1),
];

/// Commands that set up or report rather than do: left out of a key.
const NOISE: &[&str] = &[
    "cd", "pushd", "popd", "echo", "printf", "true", "false", ":", "set", "export", "source", ".",
];

/// Filters a pipe feeds: left out of a key.
const FILTERS: &[&str] = &[
    "head", "tail", "grep", "egrep", "rg", "sort", "uniq", "wc", "cut", "tr", "sed", "awk", "jq",
    "less", "more", "cat", "tee", "column", "fold",
];

/// The words after wrappers such as `env -u X`, `sudo`, `timeout 60` and
/// `mise x tool --`, and leading assignments.
fn unwrapped(words: &[String]) -> &[String] {
    let assignment = |w: &str| {
        w.split_once('=').is_some_and(|(n, _)| {
            !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    let mut w = words;
    loop {
        let Some(first) = w.first() else { return w };
        let name = first.rsplit('/').next().unwrap_or(first);
        w = match name {
            "env" => {
                let mut i = 1;
                while let Some(arg) = w.get(i) {
                    if arg == "-u" || arg == "--unset" {
                        i += 2;
                    } else if arg.starts_with('-') || assignment(arg) {
                        i += 1;
                    } else {
                        break;
                    }
                }
                w.get(i..).unwrap_or_default()
            }
            "sudo" | "nohup" | "command" | "exec" | "time" | "nice" | "rtk" => {
                let rest = &w[1..];
                let skip = rest.iter().take_while(|a| a.starts_with('-')).count();
                &rest[skip..]
            }
            "timeout" => {
                let rest = &w[1..];
                let skip = rest.iter().take_while(|a| a.starts_with('-')).count();
                rest.get(skip + 1..).unwrap_or_default()
            }
            "mise" if matches!(w.get(1).map(String::as_str), Some("x" | "exec")) => {
                match w.iter().position(|a| a == "--") {
                    Some(i) => &w[i + 1..],
                    None => return w,
                }
            }
            _ if assignment(first) => &w[1..],
            _ => return w,
        };
    }
}

/// What a command does, for grouping a timeline: each program it runs
/// with its subcommands, leaving out `cd`, `echo` and the filters it pipes
/// into, so `cd x && cargo test 2>&1 | tail -5` is `cargo test` and a dozen
/// different `grep -rn ... | head` are all `grep`.
pub(crate) fn command_key(command: &str) -> String {
    let mut keys: Vec<String> = Vec::new();
    for (words, piped) in crate::structure::simple_words(command) {
        let body = words
            .iter()
            .position(|w| !crate::structure::KEYWORDS.contains(&w.as_str()))
            .map_or(&[][..], |i| &words[i..]);
        let argv = unwrapped(body);
        let Some(first) = argv.first() else { continue };
        let program = first.rsplit('/').next().unwrap_or(first);
        if NOISE.contains(&program) || piped && FILTERS.contains(&program) {
            continue;
        }
        if matches!(program, "for" | "select" | "case") {
            if !keys.iter().any(|k| k == program) {
                keys.push(program.to_string());
            }
            continue;
        }
        let depth = SUBCOMMANDS
            .iter()
            .find(|(p, _)| *p == program)
            .map_or(0, |(_, d)| *d);
        let mut key = program.to_string();
        for word in argv[1..].iter().take(depth) {
            let plain = word.starts_with(|c: char| c.is_ascii_lowercase())
                && word.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | ':' | '_')
                });
            if !plain {
                break;
            }
            key.push(' ');
            key.push_str(word);
        }
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    match keys.len() {
        0 => shape(command),
        1..=3 => keys.join(" + "),
        _ => format!("{} + …", keys[..3].join(" + ")),
    }
}

fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Shell => "shell",
        Tool::Read => "read",
        Tool::Search => "search",
        Tool::Edit => "edit",
        Tool::Poll => "poll",
        Tool::Github => "github",
        Tool::Web => "web",
        Tool::Agent => "agent",
        Tool::Other => "other",
    }
}

/// A path inside the session's directory written relative to it, others
/// with `~` for the home directory, then redacted.
fn relative(path: &str, cwd: Option<&str>, clean: &dyn Fn(&str) -> String) -> String {
    if let Some(cwd) = cwd
        && let Some(rest) = path.strip_prefix(cwd)
        && let Some(rest) = rest.strip_prefix('/')
    {
        return clean(rest);
    }
    clean(path)
}

/// Text cut to `max` characters, with whitespace collapsed and a marker
/// saying how much was left out.
fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let total = flat.chars().count();
    if total <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{cut}… (+{} chars)", total - max)
}

/// Where each call goes: the index of the message after which it was
/// made, so calls between two prompts form a phase.
fn phases(
    t: &Transcript,
    start: Option<i64>,
    clean: &dyn Fn(&str) -> String,
    call_name: &dyn Fn(&Call) -> String,
) -> Vec<Phase> {
    let prompts: Vec<_> = t
        .messages
        .iter()
        .filter(|m| m.kind == MessageKind::Prompt)
        .collect();
    // Phase boundaries as call indexes: phase i covers calls from
    // bounds[i] to bounds[i + 1].
    let mut bounds: Vec<usize> = vec![0];
    bounds.extend(prompts.iter().map(|m| m.after_calls));
    bounds.push(t.calls.len());
    let mut out = Vec::new();
    for i in 0..bounds.len() - 1 {
        let (from, to) = (bounds[i], bounds[i + 1].max(bounds[i]));
        let prompt = i.checked_sub(1).map(|p| prompts[p]);
        let calls = &t.calls[from..to];
        if prompt.is_none() && calls.is_empty() {
            continue;
        }
        let first_line = prompt
            .map(|m| m.line)
            .or_else(|| calls.first().map(|c| c.line))
            .unwrap_or(0);
        let last_line = calls.last().map_or(first_line, |c| c.line);
        let begin = prompt.and_then(|m| m.at).or_else(|| calls.first()?.at);
        let end = calls.last().and_then(|c| {
            c.at.map(|at| {
                at + c
                    .duration_ms
                    .and_then(|d| i64::try_from(d).ok())
                    .unwrap_or(0)
            })
        });
        // A request belongs to the phase it was made in: by time, or, with
        // no time, by the first call it issued.
        let next_at = prompts.get(i).and_then(|m| m.at);
        let requests: Vec<_> = t
            .requests
            .iter()
            .filter(|r| match (r.at, prompt.and_then(|m| m.at), next_at) {
                (Some(at), begin, end) if begin.is_some() || end.is_some() => {
                    begin.is_none_or(|b| at >= b) && end.is_none_or(|e| at < e)
                }
                _ => r.calls.start >= from && r.calls.start < to,
            })
            .collect();
        out.push(Phase {
            span: Span {
                from: first_line,
                to: last_line,
            },
            offset_ms: begin.zip(start).map(|(b, s)| b - s),
            duration_ms: begin.zip(end).and_then(|(b, e)| u64::try_from(e - b).ok()),
            prompt: prompt.map(|m| excerpt(&clean(&m.text), EXCERPT)),
            requests: requests.len() as u64,
            input_tokens: requests.iter().filter_map(|r| r.input_tokens).sum(),
            calls: groups(calls, t.cwd.as_deref(), clean, call_name),
        });
    }
    out
}

/// Calls collapsed by kind, the most frequent first, then in the order
/// each kind first appears.
fn groups(
    calls: &[Call],
    cwd: Option<&str>,
    clean: &dyn Fn(&str) -> String,
    call_name: &dyn Fn(&Call) -> String,
) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut seen: Vec<Vec<String>> = Vec::new();
    for call in calls {
        let key = match (&call.command, call.tool) {
            (_, Tool::Edit) => tool_name(Tool::Edit).to_string(),
            (Some(command), _) => command_key(command),
            _ => call_name(call),
        };
        let i = *index.entry(key.clone()).or_insert_with(|| {
            out.push(Group {
                what: key,
                count: 0,
                failed: 0,
                details: Vec::new(),
                more: 0,
            });
            seen.push(Vec::new());
            out.len() - 1
        });
        let group = &mut out[i];
        group.count += 1;
        group.failed += u64::from(call.failed);
        let detail = match (&call.command, &call.target) {
            (_, Some(target)) => Some(excerpt(&relative(target, cwd, clean), 80)),
            (Some(command), None) if group.count == 1 => Some(excerpt(&clean(command), 80)),
            _ => None,
        };
        if let Some(detail) = detail
            && !seen[i].contains(&detail)
        {
            if group.details.len() < 3 {
                group.details.push(detail.clone());
            } else {
                group.more += 1;
            }
            seen[i].push(detail);
        }
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.count));
    out
}

fn corrections(
    t: &Transcript,
    clean: &dyn Fn(&str) -> String,
    call_name: &dyn Fn(&Call) -> String,
) -> Vec<Correction> {
    let mut out = Vec::new();
    let mut after_interrupt = false;
    for m in &t.messages {
        let previous = m.after_calls.checked_sub(1).and_then(|i| t.calls.get(i));
        let span = Span::at(m.line).join(previous.map_or(Span::at(m.line), |c| Span::at(c.line)));
        let after = previous.map(|c| excerpt(&call_name(c), 100));
        let (kind, cues) = match m.kind {
            MessageKind::Interrupt => (Kind::Interrupt, Vec::new()),
            MessageKind::Refusal => (Kind::Refusal, Vec::new()),
            MessageKind::Prompt => match correction::classify(&m.text) {
                Some(found) => found,
                // Whatever follows an interruption redirects the agent.
                None if after_interrupt => (Kind::Redirect, Vec::new()),
                None => {
                    after_interrupt = false;
                    continue;
                }
            },
        };
        out.push(Correction {
            span,
            kind,
            text: excerpt(&clean(&m.text), EXCERPT),
            cues,
            after,
            after_interrupt: after_interrupt && m.kind == MessageKind::Prompt,
        });
        after_interrupt = m.kind == MessageKind::Interrupt;
    }
    out
}

fn failures(
    t: &Transcript,
    clean: &dyn Fn(&str) -> String,
    call_name: &dyn Fn(&Call) -> String,
) -> Vec<Failure> {
    let mut out: Vec<Failure> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for call in &t.calls {
        let key = match (&call.target, call.tool) {
            (Some(target), Tool::Read | Tool::Search) => {
                format!(
                    "{} {}",
                    tool_name(call.tool),
                    relative(target, t.cwd.as_deref(), clean)
                )
            }
            _ => call_name(call),
        };
        match index.get(&key) {
            Some(&i) => {
                let f = &mut out[i];
                f.attempts += 1;
                f.span = f.span.join(Span::at(call.line));
                if call.failed {
                    f.failed += 1;
                    f.recovered = false;
                    if f.error.is_none() {
                        f.error = call.error.as_deref().and_then(|e| first_line(e, clean));
                    }
                } else if f.failed > 0 {
                    f.recovered = true;
                }
            }
            None if call.failed => {
                index.insert(key.clone(), out.len());
                out.push(Failure {
                    span: Span::at(call.line),
                    what: key,
                    attempts: 1,
                    failed: 1,
                    recovered: false,
                    error: call.error.as_deref().and_then(|e| first_line(e, clean)),
                    searched: crate::categorize(call) == crate::Category::ReadSearch,
                });
            }
            None => {}
        }
    }
    // A search that finds nothing fails once and means little; a failure
    // repeated, or of anything else, is listed.
    out.retain(|f| f.failed > 1 || !f.searched);
    out.sort_by(|a, b| b.failed.cmp(&a.failed).then(a.span.from.cmp(&b.span.from)));
    out
}

/// The line of an error that says what went wrong: the first naming an
/// error, else the first that is not the exit code or timing a harness
/// writes before the output.
fn first_line(error: &str, clean: &dyn Fn(&str) -> String) -> Option<String> {
    let lines: Vec<&str> = error
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("Exit code")
                && !l.starts_with("Process exited with code")
                && !l.starts_with("Chunk ID")
                && !l.starts_with("Wall time")
                && !l.starts_with("Original token count")
                && *l != "Output:"
        })
        .collect();
    let says = |l: &&&str| {
        let lower = l.to_lowercase();
        [
            "error",
            "fatal",
            "failed",
            "not found",
            "no such",
            "denied",
            "panicked",
            "cannot",
            "unknown",
            "invalid",
        ]
        .iter()
        .any(|w| lower.contains(w))
    };
    lines
        .iter()
        .find(says)
        .or(lines.first())
        .map(|l| excerpt(&clean(l), EXCERPT))
}

fn waits(t: &Transcript, call_name: &dyn Fn(&Call) -> String) -> Vec<Wait> {
    let mut out: Vec<Wait> = t
        .calls
        .iter()
        .filter(|c| c.duration_ms.is_some_and(|d| d >= LONG_CALL_MS))
        .map(|c| Wait {
            span: Span::at(c.line),
            what: call_name(c),
            duration_ms: c.duration_ms.unwrap_or(0),
        })
        .collect();
    out.sort_by_key(|w| std::cmp::Reverse(w.duration_ms));
    out
}

/// The file a call reads: a read tool's target, or the file a shell
/// command such as `cat`, `sed -n`, `head` or `nl` prints.
fn read_target(call: &Call) -> Option<String> {
    match call.tool {
        Tool::Read => call.target.clone(),
        Tool::Shell => {
            let command = call.command.as_deref()?;
            let words = crate::structure::simple_words(command);
            // The first command, or the first after a `cd`.
            let first = &words
                .iter()
                .find(|(w, _)| w.first().is_none_or(|p| p != "cd"))?
                .0;
            let program = first.first()?.rsplit('/').next()?;
            if !matches!(
                program,
                "cat" | "bat" | "less" | "more" | "head" | "tail" | "nl" | "sed"
            ) {
                return None;
            }
            if program == "sed" && !first.iter().any(|w| w == "-n") {
                return None;
            }
            first[1..]
                .iter()
                .rev()
                .find(|w| {
                    !w.starts_with('-')
                        && (w.contains('/') || w.contains('.'))
                        && !w.ends_with('p')
                        && !w.chars().all(|c| c.is_ascii_digit() || c == ',')
                })
                .cloned()
        }
        _ => None,
    }
}

fn rereads(t: &Transcript, clean: &dyn Fn(&str) -> String, d: &mut Digest) {
    struct Seen {
        reads: u64,
        unchanged: u64,
        edited: bool,
        span: Span,
        first: usize,
    }
    let mut files: HashMap<String, Seen> = HashMap::new();
    for (i, call) in t.calls.iter().enumerate() {
        if call.tool == Tool::Edit
            && let Some(target) = &call.target
            && let Some(seen) = files.get_mut(target)
        {
            seen.edited = true;
            continue;
        }
        let Some(file) = read_target(call) else {
            continue;
        };
        let seen = files.entry(file).or_insert(Seen {
            reads: 0,
            unchanged: 0,
            edited: true,
            span: Span::at(call.line),
            first: i,
        });
        seen.reads += 1;
        if !std::mem::replace(&mut seen.edited, false) {
            seen.unchanged += 1;
        }
        seen.span = seen.span.join(Span::at(call.line));
    }
    let mut listed: Vec<(String, Seen)> = files.into_iter().collect();
    listed.sort_by(|a, b| b.1.reads.cmp(&a.1.reads).then(a.1.first.cmp(&b.1.first)));
    for (file, seen) in listed {
        let name = relative(&file, t.cwd.as_deref(), clean);
        *d.reads.entry(name.clone()).or_default() += seen.reads;
        if seen.reads >= REREADS || seen.unchanged >= UNCHANGED_REREADS {
            d.rereads.push(Reread {
                file: name,
                reads: seen.reads,
                unchanged: seen.unchanged,
                span: seen.span,
            });
        }
    }
}

fn retyped(t: &Transcript, home: Option<&str>, clean: &dyn Fn(&str) -> String, d: &mut Digest) {
    let mut runs: HashMap<&str, (u64, Span, usize)> = HashMap::new();
    for (i, call) in t.calls.iter().enumerate() {
        let Some(command) = call.command.as_deref().map(str::trim) else {
            continue;
        };
        if command.len() < SHORT_COMMAND {
            continue;
        }
        let entry = runs.entry(command).or_insert((0, Span::at(call.line), i));
        entry.0 += 1;
        entry.1 = entry.1.join(Span::at(call.line));
    }
    let mut listed: Vec<_> = runs.into_iter().collect();
    listed.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.2.cmp(&b.1.2)));
    for (command, (count, span, _)) in listed {
        let shown = clean(&crate::redact::example(command, home, EXCERPT));
        if command.len() >= LONG_COMMAND || count >= 2 {
            *d.commands.entry(shown.clone()).or_default() += count;
        }
        let worth = count >= 3 || count >= 2 && command.len() >= LONG_COMMAND;
        if worth {
            d.retyped.push(Retyped {
                command: shown,
                runs: count,
                span,
            });
        }
    }
}

/// What a call's exit code was: the one recorded, or Claude Code's `Exit
/// code N` at the start of a failure's output, or 0 for a call that did
/// not fail.
fn exit_code(call: &Call) -> Option<i32> {
    if call.exit_code.is_some() {
        return call.exit_code;
    }
    if !call.failed {
        return Some(0);
    }
    let head = call.error.as_deref()?;
    let rest = head.trim_start().strip_prefix("Exit code ")?;
    rest.split_whitespace().next()?.parse().ok()
}

fn jouet_findings(t: &Transcript) -> Vec<JouetFinding> {
    let shells: Vec<&Call> = t
        .calls
        .iter()
        .filter(|c| c.tool == Tool::Shell && c.command.is_some())
        .collect();
    let runs: Vec<jouet::Run<'_>> = shells
        .iter()
        .map(|c| jouet::Run {
            command: c.command.as_deref().unwrap_or_default(),
            exit_code: exit_code(c),
            output: c.output.as_deref().or(c.error.as_deref()),
        })
        .collect();
    jouet::scan(&runs)
        .into_iter()
        .map(|found| JouetFinding {
            span: Span::at(shells[found.index].line),
            command: runs[found.index].command.to_string(),
            found,
        })
        .collect()
}

/// Where `jouet audit` puts the session's tokens and time, and the costly
/// patterns it knows.
fn attribution(harness: &str, t: &Transcript, d: &mut Digest) {
    let mut analyzer = Analyzer::default();
    analyzer.add(harness, t);
    let report = analyzer.report(1);
    let Some(h) = report.harnesses.into_iter().next() else {
        return;
    };
    d.sinks = h
        .categories
        .iter()
        .filter(|c| c.input_tokens > 0 || c.duration_ms > 0)
        .map(|c| Sink {
            category: c.category.name().to_string(),
            input_tokens: c.input_tokens,
            share: c.share,
            calls: c.calls,
            duration_ms: c.duration_ms,
            top_shape: c.top_shapes.first().map(|s| s.shape.clone()),
        })
        .collect();
    d.patterns = h
        .patterns
        .iter()
        .map(|p| KnownPattern {
            pattern: p.pattern.name().to_string(),
            calls: p.calls,
            advice: p.advice,
        })
        .collect();
}

/// What several sessions have in common: files every one of them read,
/// and commands typed out in more than one.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Across {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<Shared>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Shared>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Shared {
    pub what: String,
    pub sessions: u64,
    pub times: u64,
}

/// Files read and commands retyped in at least two of `digests`, most
/// sessions first.
pub fn across(digests: &[Digest]) -> Across {
    let collect = |pick: &dyn Fn(&Digest) -> &BTreeMap<String, u64>| {
        let mut all: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for d in digests {
            for (what, times) in pick(d) {
                let e = all.entry(what).or_default();
                e.0 += 1;
                e.1 += times;
            }
        }
        let mut out: Vec<Shared> = all
            .into_iter()
            .filter(|(_, (sessions, _))| *sessions >= 2)
            .map(|(what, (sessions, times))| Shared {
                what: what.to_string(),
                sessions,
                times,
            })
            .collect();
        out.sort_by(|a, b| b.sessions.cmp(&a.sessions).then(b.times.cmp(&a.times)));
        out
    };
    Across {
        files: collect(&|d| &d.reads),
        commands: collect(&|d| &d.commands),
    }
}

#[cfg(test)]
mod tests;
