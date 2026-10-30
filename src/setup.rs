//! Installing jouet's hooks into a harness's configuration, without touching
//! anyone else's.
//!
//! Claude Code reads hooks from `settings.json` and Codex from `hooks.json`,
//! in the same shape: `{"hooks": {EVENT: [{"matcher": ..., "hooks":
//! [HANDLER, ...]}]}}`. jouet adds one matcher group per event, holding one
//! handler whose command is `jouet hook HARNESS EVENT`. That command is how
//! jouet recognises its own entries: everything else is left as it is,
//! re-running is a no-op, and uninstalling removes only those.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

/// Seconds a harness waits for a hook. jouet's take milliseconds; this is the
/// bound on a hung one, after which both harnesses carry on without it.
const TIMEOUT: u64 = 10;

impl Harness {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// `(harness event, matcher, jouet hook event)` for each hook installed.
    ///
    /// Codex sends no exit code to its hooks, so it gets no post-tool-use
    /// hooks: its stop hook reads the session transcript instead.
    pub fn hooks(self) -> &'static [(&'static str, Option<&'static str>, &'static str)] {
        match self {
            Self::Claude => &[
                ("SessionStart", None, "session-start"),
                ("PreToolUse", Some("Bash"), "pre-tool-use"),
                ("PostToolUse", Some("Bash"), "post-tool-use"),
                ("PostToolUseFailure", Some("Bash"), "post-tool-use-failure"),
                ("Stop", None, "stop"),
            ],
            Self::Codex => &[
                ("SessionStart", None, "session-start"),
                ("PreToolUse", Some("^Bash$"), "pre-tool-use"),
                ("Stop", None, "stop"),
            ],
        }
    }

    /// The file the harness reads hooks from.
    pub fn config_path(self, scope: Scope, home: &Path, project: &Path) -> PathBuf {
        match (self, scope) {
            (Self::Claude, Scope::Global) => crate::paths::env_dir("CLAUDE_CONFIG_DIR")
                .unwrap_or_else(|| home.join(".claude"))
                .join("settings.json"),
            (Self::Claude, Scope::Project) => project.join(".claude").join("settings.json"),
            (Self::Codex, Scope::Global) => crate::paths::env_dir("CODEX_HOME")
                .unwrap_or_else(|| home.join(".codex"))
                .join("hooks.json"),
            (Self::Codex, Scope::Project) => project.join(".codex").join("hooks.json"),
        }
    }

    /// Where the harness looks for the jouet skill.
    pub fn skill_path(self, scope: Scope, home: &Path, project: &Path) -> PathBuf {
        let dir = match (self, scope) {
            (Self::Claude, Scope::Global) => crate::paths::env_dir("CLAUDE_CONFIG_DIR")
                .unwrap_or_else(|| home.join(".claude"))
                .join("skills"),
            (Self::Claude, Scope::Project) => project.join(".claude").join("skills"),
            (Self::Codex, Scope::Global) => home.join(".agents").join("skills"),
            (Self::Codex, Scope::Project) => project.join(".agents").join("skills"),
        };
        dir.join("jouet").join("SKILL.md")
    }

    fn command(self, event: &str) -> String {
        format!("jouet hook {} {event}", self.name())
    }
}

/// Whether a handler is one jouet installed: its command runs `jouet hook`.
fn is_jouet(handler: &Value) -> bool {
    let Some(command) = handler.get("command").and_then(Value::as_str) else {
        return false;
    };
    let mut words = command.split_whitespace();
    let program = words.next().map(jouet_rules::shell::basename);
    program == Some("jouet") && words.next() == Some("hook")
}

/// The matcher groups holding a jouet handler, with their events, in order.
fn jouet_groups(doc: &Value) -> Vec<(&str, &Value)> {
    let Some(hooks) = doc.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    hooks
        .iter()
        .filter_map(|(event, groups)| Some((event, groups.as_array()?)))
        .flat_map(|(event, groups)| {
            groups
                .iter()
                .filter(|g| handlers(g).iter().any(is_jouet))
                .map(move |g| (event.as_str(), g))
        })
        .collect()
}

fn handlers(group: &Value) -> &[Value] {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Checks that a document has the shape a hook file must have, so a merge
/// never replaces something it does not understand.
pub fn validate(doc: &Value) -> Result<(), String> {
    let Some(object) = doc.as_object() else {
        return Err("it is not a JSON object".into());
    };
    match object.get("hooks") {
        None => Ok(()),
        Some(Value::Object(events)) => {
            for (event, groups) in events {
                if !groups.is_array() {
                    return Err(format!("`hooks.{event}` is not an array"));
                }
            }
            Ok(())
        }
        Some(_) => Err("`hooks` is not an object".into()),
    }
}

/// Adds jouet's hooks for `harness`, replacing any jouet hooks already there.
/// Leaves the document untouched when they are already exactly as wanted.
pub fn install(doc: &mut Value, harness: Harness) {
    let before = doc.clone();
    uninstall(doc);
    let object = doc.as_object_mut().expect("validated as an object");
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("validated as an object");
    for (event, matcher, jouet_event) in harness.hooks() {
        let mut group = Map::new();
        if let Some(matcher) = matcher {
            group.insert("matcher".into(), json!(matcher));
        }
        group.insert(
            "hooks".into(),
            json!([{"type": "command", "command": harness.command(jouet_event), "timeout": TIMEOUT}]),
        );
        hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("validated as an array")
            .push(Value::Object(group));
    }
    // A re-run would move jouet's groups after any added since; when they
    // are already right, where they sit is left alone.
    let only_jouet = |doc: &Value| {
        jouet_groups(doc)
            .iter()
            .all(|(_, g)| handlers(g).iter().all(is_jouet))
    };
    if only_jouet(&before) && jouet_groups(&before) == jouet_groups(doc) {
        *doc = before;
    }
}

/// Removes every jouet handler, then any group, event or `hooks` object that
/// removing them left empty.
pub fn uninstall(doc: &mut Value) {
    let Some(object) = doc.as_object_mut() else {
        return;
    };
    let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    hooks.retain(|_, groups| {
        let Some(list) = groups.as_array_mut() else {
            return true;
        };
        let had = list.len();
        list.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|h| !is_jouet(h));
            !(handlers.is_empty() && before > 0)
        });
        !(list.is_empty() && had > 0)
    });
    if hooks.is_empty() {
        object.remove("hooks");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(mut doc: Value, harness: Harness) -> Value {
        validate(&doc).unwrap();
        install(&mut doc, harness);
        doc
    }

    fn pretty(doc: &Value) -> String {
        serde_json::to_string_pretty(doc).unwrap()
    }

    #[test]
    fn installs_into_an_empty_file() {
        let doc = installed(json!({}), Harness::Codex);
        assert_eq!(
            pretty(&doc),
            r#"{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook codex session-start",
            "timeout": 10
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "^Bash$",
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook codex pre-tool-use",
            "timeout": 10
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook codex stop",
            "timeout": 10
          }
        ]
      }
    ]
  }
}"#
        );
    }

    /// A settings file with other settings, another tool's hooks for some of
    /// the same events, and a group that mixes a jouet handler with someone
    /// else's.
    fn existing() -> Value {
        json!({
            "theme": "dark",
            "hooks": {
                "Stop": [
                    {"hooks": [{"type": "command", "command": "entire hooks claude-code stop"}]}
                ],
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [
                        {"type": "command", "command": "my-guard"},
                        {"type": "command", "command": "jouet hook claude pre-tool-use"}
                    ]}
                ],
                "Notification": [
                    {"hooks": [{"type": "command", "command": "notify-send hi"}]}
                ]
            },
            "permissions": {"allow": ["Bash(ls)"]}
        })
    }

    #[test]
    fn merging_keeps_everything_else_in_place() {
        let doc = installed(existing(), Harness::Claude);
        assert_eq!(
            pretty(&doc),
            r#"{
  "theme": "dark",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "entire hooks claude-code stop"
          }
        ]
      },
      {
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook claude stop",
            "timeout": 10
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "my-guard"
          }
        ]
      },
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook claude pre-tool-use",
            "timeout": 10
          }
        ]
      }
    ],
    "Notification": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "notify-send hi"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook claude session-start",
            "timeout": 10
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook claude post-tool-use",
            "timeout": 10
          }
        ]
      }
    ],
    "PostToolUseFailure": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "jouet hook claude post-tool-use-failure",
            "timeout": 10
          }
        ]
      }
    ]
  },
  "permissions": {
    "allow": [
      "Bash(ls)"
    ]
  }
}"#
        );
    }

    #[test]
    fn rerunning_is_a_no_op() {
        let once = installed(existing(), Harness::Claude);
        assert_eq!(installed(once.clone(), Harness::Claude), once);

        // Even when another tool has since added hooks after jouet's.
        let mut later = once.clone();
        later["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks": [{"type": "command", "command": "late"}]}));
        assert_eq!(installed(later.clone(), Harness::Claude), later);
    }

    #[test]
    fn an_older_jouet_entry_is_replaced() {
        let doc = json!({"hooks": {"PreToolUse": [
            {"matcher": "Bash", "hooks": [{"type": "command", "command": "/old/bin/jouet hook claude pre-tool-use --old"}]}
        ]}});
        let doc = installed(doc, Harness::Claude);
        let pre = &doc["hooks"]["PreToolUse"];
        assert_eq!(pre.as_array().unwrap().len(), 1);
        assert_eq!(
            pre[0]["hooks"][0]["command"],
            "jouet hook claude pre-tool-use"
        );
    }

    #[test]
    fn uninstall_removes_only_what_jouet_added() {
        let mut doc = installed(existing(), Harness::Claude);
        uninstall(&mut doc);
        let mut expected = existing();
        // The mixed group keeps the other tool's handler.
        expected["hooks"]["PreToolUse"][0]["hooks"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(doc, expected);

        let mut only_jouet = installed(json!({}), Harness::Codex);
        uninstall(&mut only_jouet);
        assert_eq!(only_jouet, json!({}));
    }

    #[test]
    fn uninstall_leaves_empty_groups_it_did_not_empty() {
        let original = json!({"hooks": {"Stop": [], "PreToolUse": [{"hooks": []}]}});
        let mut doc = original.clone();
        uninstall(&mut doc);
        assert_eq!(doc, original);
    }

    #[test]
    fn refuses_shapes_it_does_not_understand() {
        assert!(validate(&json!([])).is_err());
        assert!(validate(&json!({"hooks": []})).is_err());
        assert!(validate(&json!({"hooks": {"Stop": {}}})).is_err());
        assert!(validate(&json!({"hooks": {"Stop": []}})).is_ok());
    }

    #[test]
    fn recognises_jouet_commands_by_program_and_subcommand() {
        assert!(is_jouet(&json!({"command": "jouet hook claude stop"})));
        assert!(is_jouet(
            &json!({"command": "/usr/local/bin/jouet hook codex stop"})
        ));
        assert!(!is_jouet(&json!({"command": "jouet edit"})));
        assert!(!is_jouet(&json!({"command": "echo jouet hook"})));
        assert!(!is_jouet(&json!({"type": "prompt"})));
    }
}
