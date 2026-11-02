//! Which agent within a session made a call.
//!
//! Claude Code runs its subagents (the Agent tool) inside the lead's session:
//! their hook payloads carry the lead's `session_id`, plus an `agent_id` the
//! lead's own never have, and the commands they run get only the session's
//! id in their environment (`CLAUDE_CODE_SESSION_ID`). So what a session was
//! shown, and which work trees it is answerable for at a stop, is kept per
//! agent: the hooks read the agent from the payload, and a jouet command run
//! from a Bash call looks up the call the pre-tool-use hook noted for it.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// How long a noted call counts as running: past Claude Code's longest
/// foreground Bash call, with room to spare.
const CALL_KEPT_SECS: i64 = 15 * 60;

/// Who in a session ran a jouet command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Caller {
    /// The session's own agent, or a command no hook noted (run outside a
    /// hooked harness, or with `JOUET_SESSION_ID` set by hand).
    Lead,
    /// A subagent, by the `agent_id` its hooks carry.
    Agent(String),
    /// More than one agent was running a jouet call when it started, so it
    /// cannot tell whose it is.
    Unknown,
}

impl Caller {
    /// Whether this is the session's own agent.
    pub(crate) fn is_lead(&self) -> bool {
        *self == Self::Lead
    }
}

/// The subagent a hook payload is from; None for the lead's.
pub(crate) fn of_payload(payload: &Value) -> Option<&str> {
    payload
        .get("agent_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// What a session's memory of what it was shown is kept under: the session
/// for its lead, the session and agent for a subagent.
pub(crate) fn key(session: &str, agent: Option<&str>) -> String {
    match agent {
        Some(agent) => format!("{session}-agent-{agent}"),
        None => session.to_string(),
    }
}

/// The memory key for the jouet `subcommand` running in `session`, by who
/// ran it; None when that cannot be told, so nothing is remembered or
/// collapsed.
pub(crate) fn command_key(session: &str, subcommand: &str) -> Option<String> {
    match caller(session, subcommand) {
        Caller::Lead => Some(key(session, None)),
        Caller::Agent(agent) => Some(key(session, Some(&agent))),
        Caller::Unknown => None,
    }
}

/// A Bash call that runs jouet, as the pre-tool-use hook noted it.
#[derive(Debug, Serialize, Deserialize)]
struct Noted {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    command: String,
    at: i64,
}

/// Notes a Bash call about to run jouet, with the agent making it, so the
/// command can tell who ran it. Calls that do not mention jouet are not
/// noted.
pub(crate) fn note_call(payload: &Value) {
    let Some((path, command)) = call_path(payload) else {
        return;
    };
    let noted = Noted {
        agent: of_payload(payload).map(str::to_string),
        command: command.to_string(),
        at: jouet_output::time::now(),
    };
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(&noted) {
        let _ = std::fs::write(&path, text);
    }
}

/// Forgets a noted call once it has finished. A call moved to the background
/// is still running, so it is kept until it is [`CALL_KEPT_SECS`] old.
pub(crate) fn end_call(payload: &Value) {
    let background = payload
        .get("tool_input")
        .and_then(|i| i.get("run_in_background"))
        .and_then(Value::as_bool)
        == Some(true);
    if let (false, Some((path, _))) = (background, call_path(payload)) {
        let _ = std::fs::remove_file(path);
    }
}

/// Where the call in `payload` is noted, and its command, for a Bash call
/// that mentions jouet.
fn call_path(payload: &Value) -> Option<(PathBuf, &str)> {
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return None;
    }
    let command = payload
        .get("tool_input")?
        .get("command")?
        .as_str()
        .filter(|c| c.contains("jouet"))?;
    let session = safe(payload.get("session_id")?.as_str()?)?;
    let call = safe(payload.get("tool_use_id")?.as_str()?)?;
    let dir = calls_dir()?.join(session);
    Some((dir.join(format!("{call}.json")), command))
}

/// `<state dir>/calls`.
fn calls_dir() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("calls"))
}

fn safe(id: &str) -> Option<String> {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    (!safe.is_empty()).then_some(safe)
}

/// Who in `session` ran this jouet command, `subcommand`, worked out once
/// per process.
pub(crate) fn caller(session: &str, subcommand: &str) -> Caller {
    static CALLER: OnceLock<Caller> = OnceLock::new();
    CALLER
        .get_or_init(|| {
            let Some(dir) = calls_dir().zip(safe(session)).map(|(d, s)| d.join(s)) else {
                return Caller::Lead;
            };
            caller_in(&dir, subcommand, jouet_output::time::now())
        })
        .clone()
}

/// Who ran a jouet `subcommand`, from the calls noted in `dir` that are
/// running at `now` and could have run it. Older calls are removed.
fn caller_in(dir: &Path, subcommand: &str, now: i64) -> Caller {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Caller::Lead;
    };
    let mut agents: Vec<Option<String>> = Vec::new();
    for entry in entries.flatten() {
        let Some(noted) = std::fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Noted>(&bytes).ok())
        else {
            continue;
        };
        if now.saturating_sub(noted.at) > CALL_KEPT_SECS {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        let runs = noted
            .command
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .any(|word| word == subcommand);
        if runs && !agents.contains(&noted.agent) {
            agents.push(noted.agent);
        }
    }
    match agents.as_slice() {
        [] | [None] => Caller::Lead,
        [Some(agent)] => Caller::Agent(agent.clone()),
        _ => Caller::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(dir: &Path, call: &str, agent: Option<&str>, command: &str, at: i64) {
        std::fs::create_dir_all(dir).unwrap();
        let noted = Noted {
            agent: agent.map(str::to_string),
            command: command.into(),
            at,
        };
        std::fs::write(
            dir.join(format!("{call}.json")),
            serde_json::to_string(&noted).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_subagent_is_kept_apart_from_its_session() {
        assert_eq!(key("s1", None), "s1");
        assert_eq!(key("s1", Some("a7f3")), "s1-agent-a7f3");
        let payload = serde_json::json!({"session_id": "s1", "agent_id": "a7f3"});
        assert_eq!(of_payload(&payload), Some("a7f3"));
        assert_eq!(of_payload(&serde_json::json!({"session_id": "s1"})), None);
    }

    #[test]
    fn a_command_is_the_agent_s_whose_call_runs_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s1");
        assert_eq!(caller_in(&dir, "read", 1000), Caller::Lead);

        note(
            &dir,
            "t1",
            Some("a1"),
            "jouet read src/lib.rs --symbol x",
            990,
        );
        note(
            &dir,
            "t2",
            None,
            "cargo build && jouet test --affected",
            995,
        );
        assert_eq!(caller_in(&dir, "read", 1000), Caller::Agent("a1".into()));
        assert_eq!(caller_in(&dir, "test", 1000), Caller::Lead);
        assert_eq!(caller_in(&dir, "search", 1000), Caller::Lead);

        // Two agents running a read at once: it cannot tell whose.
        note(&dir, "t3", Some("a2"), "jouet read src/main.rs", 998);
        assert_eq!(caller_in(&dir, "read", 1000), Caller::Unknown);

        // A call noted long ago is no longer running, and is removed.
        let later = 998 + CALL_KEPT_SECS + 1;
        assert_eq!(caller_in(&dir, "read", later), Caller::Lead);
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    }
}
