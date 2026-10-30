//! Noticing costly patterns in the ledger, one entry at a time, so `jouet
//! wind` only reads what is new and keeps what it has seen in a small state.
//!
//! The ledger holds command *shapes*: programs, subcommands and flag names,
//! with every value a placeholder. Two patterns show in shapes, and each is
//! something a rule can catch before the call runs:
//!
//! - **never works**: a command (the same program, subcommands and flags)
//!   that has failed again and again, across sessions, and never succeeded,
//!   such as a tool that is not installed. Commands a post-tool-use rule
//!   counts as a test, lint or build run are left out: those fail while code
//!   is being fixed.
//! - **sleep**: long `sleep`s the agent waits out, by the time they took,
//!   instead of waiting for what it needs.

use super::reader;
use crate::ledger::Entry;
use jouet_rules::shell;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A sleep that took at least this long counts.
pub const LONG_SLEEP_MS: u64 = 10_000;
/// How many later calls in the session are looked at for what worked
/// instead of a failure.
const FIX_WINDOW: u8 = 3;
const MAX_EXAMPLES: usize = 5;
const MAX_SESSIONS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pattern {
    NeverWorks,
    Sleep,
}

impl Pattern {
    /// Occurrences, and sessions, before a pattern is worth a rule.
    fn threshold(self) -> (u64, usize) {
        match self {
            Self::NeverWorks => (3, 2),
            Self::Sleep => (5, 2),
        }
    }
}

/// What identifies a command in a shape: its program and up to two
/// subcommands, and the names of its flags.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Signature {
    pub words: Vec<String>,
    pub flags: Vec<String>,
}

impl Signature {
    pub fn of(command: &shell::Command) -> Self {
        let argv = command.argv();
        let mut words: Vec<String> = argv
            .first()
            .map(|p| shell::basename(p).to_string())
            .into_iter()
            .collect();
        let mut flags = BTreeSet::new();
        let mut positional = false;
        for word in argv.iter().skip(1) {
            if word == "--" {
                break;
            }
            if word.starts_with('-') && word.len() > 1 {
                let name = word.split_once('=').map_or(word.as_str(), |(n, _)| n);
                flags.insert(name.to_string());
                continue;
            }
            let subcommand = word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !positional && subcommand && words.len() < 3 && flags.is_empty() {
                words.push(word.clone());
            } else {
                positional = true;
            }
        }
        Self {
            words,
            flags: flags.into_iter().collect(),
        }
    }

    pub fn text(&self) -> String {
        let mut parts = self.words.clone();
        parts.extend(self.flags.iter().cloned());
        parts.join(" ")
    }
}

/// What has been seen of one pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub pattern: Pattern,
    pub signature: Signature,
    pub count: u64,
    pub sessions: BTreeSet<String>,
    /// Distinct shapes that showed the pattern, oldest first.
    pub examples: Vec<String>,
    /// For a failure: the shape of a command that worked instead, later in
    /// a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Failed calls, or milliseconds slept, summed.
    #[serde(default)]
    pub cost: u64,
    pub first: u64,
    pub last: u64,
}

impl Evidence {
    pub fn ready(&self) -> bool {
        let (count, sessions) = self.pattern.threshold();
        self.count >= count && self.sessions.len() >= sessions
    }
}

/// A failure waiting to see what the session did next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    key: String,
    program: String,
    shape: String,
    left: u8,
    ts: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detector {
    #[serde(default)]
    pub evidence: BTreeMap<String, Evidence>,
    /// Signatures that have succeeded at least once, which rules out "never
    /// works" for them.
    #[serde(default)]
    worked: BTreeSet<String>,
    #[serde(default)]
    pending: BTreeMap<String, Pending>,
    /// Per session, the shape of the last call a rule spoke about: its
    /// pattern is covered already.
    #[serde(default)]
    covered: BTreeMap<String, String>,
}

impl Detector {
    /// Takes in one ledger entry. `is_check` says whether a script is a
    /// test, lint or build run, whose failures are ordinary.
    pub fn observe(&mut self, entry: &Entry, is_check: &dyn Fn(&str) -> bool) {
        if entry.tool.as_deref() != Some("Bash") {
            return;
        }
        let session = entry.session.clone().unwrap_or_default();
        let Some(shape) = entry.shape.as_deref() else {
            return;
        };
        let spoke = entry
            .decisions
            .iter()
            .any(|d| d.action == "deny" || d.action == "warn");
        if entry.event == "pre-tool-use" {
            if spoke {
                self.covered.insert(session, shape.to_string());
            }
            return;
        }
        let Some(exit_code) = entry.exit_code.filter(|_| entry.event == "post-tool-use") else {
            return;
        };
        if self.covered.get(&session).is_some_and(|s| s == shape) {
            self.covered.remove(&session);
            return;
        }
        let script = reader::script(shape);
        let parsed = shell::parse(&script);
        self.settle(&session, &parsed, shape, exit_code);
        let statuses = shell::statuses(&parsed.commands, exit_code);
        let check = exit_code != 0 && is_check(&script);
        for (command, status) in parsed.commands.iter().zip(&statuses) {
            let signature = Signature::of(command);
            if signature.words.is_empty() {
                continue;
            }
            let key = signature.text();
            match status {
                Some(0) => {
                    self.worked.insert(key.clone());
                    self.evidence.remove(&format!("never-works:{key}"));
                }
                Some(code) if !check && !self.worked.contains(&key) => {
                    let ekey = format!("never-works:{key}");
                    let program = signature.words[0].clone();
                    let evidence = self.add(
                        &ekey,
                        Pattern::NeverWorks,
                        signature,
                        &session,
                        entry.ts,
                        shape,
                    );
                    evidence.exit_code = Some(*code);
                    evidence.cost += 1;
                    self.pending.insert(
                        session.clone(),
                        Pending {
                            key: ekey,
                            program,
                            shape: shape.to_string(),
                            left: FIX_WINDOW,
                            ts: entry.ts,
                        },
                    );
                }
                _ => {}
            }
        }
        let sleeps = parsed.commands.iter().any(|c| c.program() == Some("sleep"));
        if let Some(ms) = entry.duration_ms
            && sleeps
            && ms >= LONG_SLEEP_MS
        {
            let signature = Signature {
                words: vec!["sleep".into()],
                flags: Vec::new(),
            };
            let evidence = self.add(
                "sleep",
                Pattern::Sleep,
                signature,
                &session,
                entry.ts,
                shape,
            );
            evidence.cost += ms;
        }
    }

    /// Looks at a session's next call after a failure: a different command
    /// running the same program that succeeds is what worked instead.
    fn settle(&mut self, session: &str, parsed: &shell::Script, shape: &str, exit_code: i32) {
        let Some(pending) = self.pending.get(session).cloned() else {
            return;
        };
        let same_program = parsed
            .commands
            .iter()
            .any(|c| c.program() == Some(pending.program.as_str()));
        if exit_code == 0 && same_program && shape != pending.shape {
            if let Some(evidence) = self.evidence.get_mut(&pending.key) {
                evidence.fix = Some(shape.to_string());
            }
            self.pending.remove(session);
        } else if pending.left <= 1 {
            self.pending.remove(session);
        } else if let Some(p) = self.pending.get_mut(session) {
            p.left -= 1;
        }
    }

    fn add(
        &mut self,
        key: &str,
        pattern: Pattern,
        signature: Signature,
        session: &str,
        ts: u64,
        shape: &str,
    ) -> &mut Evidence {
        let evidence = self
            .evidence
            .entry(key.to_string())
            .or_insert_with(|| Evidence {
                pattern,
                signature,
                count: 0,
                sessions: BTreeSet::new(),
                examples: Vec::new(),
                fix: None,
                exit_code: None,
                cost: 0,
                first: ts,
                last: ts,
            });
        evidence.count += 1;
        evidence.last = ts;
        if evidence.sessions.len() < MAX_SESSIONS {
            evidence.sessions.insert(session.to_string());
        }
        if evidence.examples.len() < MAX_EXAMPLES && !evidence.examples.iter().any(|e| e == shape) {
            evidence.examples.push(shape.to_string());
        }
        evidence
    }

    /// Called when a run ends: forgets what is over an hour old and still
    /// waiting for the session's next call, so the state stays small.
    pub fn finish(&mut self, now: u64) {
        self.pending.retain(|_, p| now.saturating_sub(p.ts) < 3600);
        self.covered.clear();
    }

    /// Evidence that has reached its threshold.
    pub fn ready(&self) -> impl Iterator<Item = (&String, &Evidence)> {
        self.evidence.iter().filter(|(_, e)| e.ready())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ledger::Decision;

    pub fn ran(ts: u64, session: &str, script: &str, exit_code: i32) -> Entry {
        Entry {
            ts,
            harness: "claude".into(),
            session: Some(session.into()),
            event: "post-tool-use".into(),
            tool: Some("Bash".into()),
            shape: Some(crate::ledger::shape(script)),
            exit_code: Some(exit_code),
            ..Entry::default()
        }
    }

    fn never(_: &str) -> bool {
        false
    }

    fn signature(text: &str) -> String {
        let script = reader::script(&crate::ledger::shape(text));
        Signature::of(&shell::parse(&script).commands[0]).text()
    }

    #[test]
    fn signatures() {
        assert_eq!(signature("gh pr view 12 --json x"), "gh pr view --json");
        assert_eq!(signature("cargo test -p x --all"), "cargo test --all -p");
        assert_eq!(signature("ls -la"), "ls -la");
        assert_eq!(signature("/usr/bin/git -C /x push -f origin"), "git -C -f");
        assert_eq!(
            signature("env A=1 timeout 5 git push -u origin"),
            "git push -u"
        );
    }

    #[test]
    fn a_command_that_never_works_is_noticed_with_what_worked() {
        let mut d = Detector::default();
        d.observe(&ran(1, "a", "mytool sync --all", 127), &never);
        d.observe(&ran(2, "a", "mytool pull", 0), &never);
        d.observe(&ran(3, "a", "other thing", 127), &never);
        assert_eq!(d.ready().count(), 0);
        d.observe(&ran(4, "b", "cd x && mytool sync --all", 127), &never);
        d.observe(&ran(5, "b", "mytool sync --all", 127), &never);
        let ready: Vec<_> = d.ready().map(|(k, _)| k.as_str()).collect();
        assert_eq!(ready, ["never-works:mytool sync --all"]);
        let e = &d.evidence["never-works:mytool sync --all"];
        assert_eq!(e.fix.as_deref(), Some("mytool pull"));
        assert_eq!(e.exit_code, Some(127));
        assert_eq!(
            e.examples,
            ["mytool sync --all", "cd x && mytool sync --all"]
        );
    }

    #[test]
    fn a_command_that_ever_worked_or_is_a_check_is_left_alone() {
        let mut d = Detector::default();
        for (i, s) in ["a", "b", "c"].iter().enumerate() {
            d.observe(&ran(i as u64, s, "gh pr view 1", 1), &never);
        }
        d.observe(&ran(9, "c", "gh pr view 2", 0), &never);
        assert_eq!(d.ready().count(), 0);
        let checks = |s: &str| s.starts_with("cargo test");
        for (i, s) in ["a", "b", "c"].iter().enumerate() {
            d.observe(&ran(i as u64, s, "cargo test", 101), &checks);
        }
        assert_eq!(d.ready().count(), 0);
    }

    #[test]
    fn long_sleeps_by_how_long_they_took() {
        let mut d = Detector::default();
        for (i, s) in ["a", "a", "b", "b", "b"].iter().enumerate() {
            let mut long = ran(i as u64, s, "sleep 120", 0);
            long.duration_ms = Some(120_000);
            d.observe(&long, &never);
            let mut short = ran(i as u64, s, "sleep 2", 0);
            short.duration_ms = Some(2_000);
            d.observe(&short, &never);
        }
        let ready: Vec<_> = d.ready().map(|(k, _)| k.as_str()).collect();
        assert_eq!(ready, ["sleep"]);
        assert_eq!(d.evidence["sleep"].cost, 600_000);
    }

    #[test]
    fn a_call_a_rule_spoke_about_is_covered() {
        let mut d = Detector::default();
        for (i, s) in ["a", "b", "c"].iter().enumerate() {
            let mut pre = ran(i as u64, s, "nope x", 0);
            pre.event = "pre-tool-use".into();
            pre.exit_code = None;
            pre.decisions = vec![Decision {
                rule: "r".into(),
                action: "warn".into(),
            }];
            d.observe(&pre, &never);
            d.observe(&ran(i as u64, s, "nope x", 127), &never);
        }
        assert_eq!(d.ready().count(), 0);
    }
}
