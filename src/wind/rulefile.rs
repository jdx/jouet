//! Changing a user's rule files: writing a new one, switching its mode,
//! retiring it, and the log every such change is written to.
//!
//! Only rules in `~/.config/jouet/rules` are ever written. Built-in rules are
//! part of the binary, and a repository's rules change through a reviewed PR.

use jouet_rules::Mode;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Whether a name is fit for a rule file: lowercase words joined by dashes.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-')
}

pub struct Header<'a> {
    pub name: &'a str,
    pub why: &'a str,
    pub author: &'a str,
    pub created: &'a str,
}

/// A rule's header: always in warn mode and user scope, since that is the
/// only kind of rule jouet writes.
pub fn header(h: &Header) -> String {
    format!(
        "--!strict\n--- name: {}\n--- why: {}\n--- mode: warn\n--- scope: user\n--- author: {}\n--- created: {}\n",
        h.name,
        one_line(h.why),
        one_line(h.author),
        h.created
    )
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A new rule with a header and a `check` to fill in.
pub fn scaffold(h: &Header) -> String {
    format!(
        r#"{}
-- `jouet rule types` prints the API; `jouet rule test {name} --replay` runs this
-- rule over the calls jouet's hooks have seen.
local rule: JouetRule = {{
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			-- Match the command this rule is about, for example:
			-- if command.program == "git" and command.argv[2] == "stash" then
			if false then
				return jouet.deny("Say what to do instead.")
			end
		end
		return nil
	end,
	examples = {{
		-- Commands the rule must stop, and near misses it must allow.
		deny = {{}},
		allow = {{}},
	}},
}}

return rule
"#,
        header(h),
        name = h.name
    )
}

/// The rule's code with its header's mode set, adding the line after `why`
/// when there is none.
pub fn set_mode(code: &str, mode: Mode) -> String {
    let word = match mode {
        Mode::Warn => "warn",
        Mode::Deny => "deny",
    };
    set_field(code, "mode", word)
}

/// The rule's code with one header field set, adding it after `why` when the
/// header has none. A file without a header is returned as it is.
pub fn set_field(code: &str, key: &str, value: &str) -> String {
    let line = format!("--- {key}: {}\n", one_line(value));
    // The header's lines, with each one's field name, up to the first line
    // that is not part of it.
    let lines: Vec<&str> = code.split_inclusive('\n').collect();
    let field = |l: &str| {
        let t = l.trim();
        t.strip_prefix("---")
            .and_then(|f| f.split_once(':'))
            .map(|(k, _)| k.trim().to_string())
    };
    let header_len = lines
        .iter()
        .position(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("--!") && field(l).is_none()
        })
        .unwrap_or(lines.len());
    let header = &lines[..header_len];
    let at = header
        .iter()
        .position(|l| field(l).as_deref() == Some(key))
        .map(|i| (i, true))
        .or_else(|| {
            header
                .iter()
                .position(|l| field(l).as_deref() == Some("why"))
                .map(|i| (i + 1, false))
        });
    let Some((at, replace)) = at else {
        return code.to_string();
    };
    let mut out: String = lines[..at].concat();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line);
    let rest = if replace { at + 1 } else { at };
    out.push_str(&lines[rest..].concat());
    out
}

/// Where retired rules go: beside the rules, where they no longer load.
pub fn retired_dir(rules_dir: &Path) -> PathBuf {
    rules_dir.join("retired")
}

/// Moves a rule out of the directory rules load from. Returns where it went.
pub fn retire(path: &Path) -> std::io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let retired = retired_dir(dir);
    std::fs::create_dir_all(&retired)?;
    let name = path.file_name().unwrap_or_default();
    let mut target = retired.join(name);
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = retired.join(format!("{}.{n}", name.to_string_lossy()));
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// One change to a rule, as `jouet rule log` prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub ts: u64,
    pub rule: String,
    /// `created`, `promoted`, `retired` or `shared`.
    pub change: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub reason: String,
    /// Who made the change: a session, `user`, or `jouet wind`.
    pub by: String,
}

/// `$XDG_STATE_HOME/jouet/rule-log.jsonl`.
pub fn log_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("rule-log.jsonl"))
}

pub fn log(path: &Path, change: &Change) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_vec(change)?;
    line.push(b'\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)
}

/// Every change in the log, oldest first; unreadable lines are skipped.
pub fn read_log(path: &Path) -> Vec<Change> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Header = Header {
        name: "my-rule",
        why: "because\nreasons",
        author: "session-1",
        created: "2026-10-30",
    };

    #[test]
    fn names() {
        for good in ["a", "wind-cargo-127", "x9"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-a", "a-", "A", "a_b", "a/b", "../x", "9a"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_scaffold_loads_in_warn_mode_with_its_header() {
        let code = scaffold(&H);
        assert!(code.contains("--- why: because reasons\n"), "{code}");
        let engine = jouet_rules::Engine::new(
            vec![jouet_rules::Source {
                name: "my-rule".into(),
                origin: jouet_rules::Origin::User("my-rule.luau".into()),
                code,
                trusted: true,
            }],
            jouet_rules::Options {
                has_command: Box::new(|_| true),
                disabled: Vec::new(),
                budget: jouet_rules::Budget::default(),
                cache_dir: None,
            },
        )
        .unwrap();
        let rule = engine.rule("my-rule").unwrap();
        assert_eq!(rule.state, jouet_rules::State::Enabled);
        assert_eq!(rule.mode, Mode::Warn);
        assert_eq!(rule.author, "session-1");
    }

    #[test]
    fn set_mode_rewrites_or_adds_the_header_line() {
        let code = scaffold(&H);
        let deny = set_mode(&code, Mode::Deny);
        assert!(deny.contains("--- mode: deny\n") && !deny.contains("mode: warn"));
        assert_eq!(set_mode(&deny, Mode::Warn), code);
        let renamed = set_field(&code, "author", "session-9");
        assert!(renamed.contains("--- author: session-9\n") && !renamed.contains("session-1"));
        let bare = "--- why: x\nreturn {}\n-- --- mode: warn\n";
        assert_eq!(
            set_mode(bare, Mode::Deny),
            "--- why: x\n--- mode: deny\nreturn {}\n-- --- mode: warn\n"
        );
    }

    #[test]
    fn retiring_moves_the_file_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.luau");
        std::fs::write(&path, "x").unwrap();
        let first = retire(&path).unwrap();
        std::fs::write(&path, "y").unwrap();
        let second = retire(&path).unwrap();
        assert_ne!(first, second);
        assert!(!path.exists());
        assert!(jouet_rules::rule_files(dir.path()).unwrap().is_empty());
    }
}
