//! Replaying a rule over the ledger's real calls: which ones it would have
//! fired on, and the near misses it stays quiet on, the calls running the
//! same programs as the ones it fires on.
//!
//! The ledger keeps shapes, not commands, so a rule sees each call with its
//! values as placeholder words (`git commit -m ARG`): a rule that matches on
//! programs, subcommands and flags replays exactly, and one that reads a
//! value sees `ARG`.

use super::reader;
use crate::ledger::Entry;
use jouet_rules::{Action, Call, Engine, Event, Rule, shell};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// How many fired calls and near misses a report lists.
pub const SHOWN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hit {
    pub command: String,
    /// How many times the ledger holds this call.
    pub count: u64,
    /// `deny`, `warn`, `record test` and so on; for a near miss, `allow`.
    pub verdict: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Replay {
    /// Calls replayed: the shell calls of the rule's event.
    pub calls: u64,
    /// Calls the rule fired on, most frequent first.
    pub fired: Vec<Hit>,
    /// Distinct calls the rule fired on, before [`SHOWN`] cut the list.
    pub fired_distinct: usize,
    pub fired_total: u64,
    /// Calls it stayed quiet on that run a program it fired on.
    pub near_misses: Vec<Hit>,
    pub near_misses_distinct: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// The programs a shape runs, by basename.
fn programs(shape: &str) -> BTreeSet<String> {
    shell::parse(&reader::script(shape))
        .all()
        .into_iter()
        .filter_map(|c| c.program().map(str::to_string))
        .collect()
}

/// Runs the rule on every shell call of its event the entries hold.
pub fn replay(engine: &Engine, rule: &Rule, entries: &[Entry]) -> Replay {
    let mut report = Replay::default();
    let mut fired: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut quiet: BTreeMap<String, u64> = BTreeMap::new();
    let mut errors = BTreeSet::new();
    for entry in entries {
        let matches_event = match rule.event {
            Event::PreToolUse => entry.event == "pre-tool-use",
            Event::PostToolUse => entry.event == "post-tool-use" && entry.exit_code.is_some(),
        };
        let Some(command) = entry.shape.as_deref() else {
            continue;
        };
        if !matches_event || entry.tool.as_deref() != Some("Bash") {
            continue;
        }
        report.calls += 1;
        let call = Call {
            event: Some(rule.event),
            harness: entry.harness.clone(),
            tool: "Bash".into(),
            command: Some(reader::script(command)),
            cwd: None,
            exit_code: entry.exit_code,
        };
        match engine.check(rule, &call) {
            Ok((decision, records)) => {
                let spoke = !records.is_empty()
                    || decision
                        .as_ref()
                        .is_some_and(|d| matches!(d.action, Action::Deny | Action::Warn));
                if spoke {
                    let verdict = jouet_rules::summarize(decision.as_ref(), &records);
                    fired.entry(command.to_string()).or_insert((0, verdict)).0 += 1;
                    report.fired_total += 1;
                } else {
                    *quiet.entry(command.to_string()).or_default() += 1;
                }
            }
            Err(e) => {
                errors.insert(e);
            }
        }
    }
    let fired_programs: BTreeSet<String> = fired.keys().flat_map(|c| programs(c)).collect();
    let mut near: Vec<Hit> = quiet
        .into_iter()
        .filter(|(command, _)| !programs(command).is_disjoint(&fired_programs))
        .map(|(command, count)| Hit {
            command,
            count,
            verdict: "allow".into(),
        })
        .collect();
    let mut hits: Vec<Hit> = fired
        .into_iter()
        .map(|(command, (count, verdict))| Hit {
            command,
            count,
            verdict,
        })
        .collect();
    hits.sort_by(|a, b| b.count.cmp(&a.count).then(a.command.cmp(&b.command)));
    near.sort_by(|a, b| b.count.cmp(&a.count).then(a.command.cmp(&b.command)));
    report.fired_distinct = hits.len();
    report.near_misses_distinct = near.len();
    hits.truncate(SHOWN);
    near.truncate(SHOWN);
    report.fired = hits;
    report.near_misses = near;
    report.errors = errors.into_iter().collect();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wind::stats::tests::fired;
    use jouet_rules::{Budget, Options, Origin, Source};

    pub fn engine(name: &str, code: &str) -> Engine {
        Engine::new(
            vec![Source {
                name: name.into(),
                origin: Origin::User(format!("{name}.luau").into()),
                code: code.into(),
                trusted: true,
            }],
            Options {
                has_command: Box::new(|_| true),
                disabled: Vec::new(),
                budget: Budget::default(),
                cache_dir: None,
            },
        )
        .unwrap()
    }

    const NO_TAGS: &str = r#"--- why: tags
return { check = function(call)
	for _, c in call.commands do
		if c.program == "git" and c.argv[2] == "tag" then return jouet.deny("no tags") end
	end
	return nil
end }"#;

    #[test]
    fn reports_what_it_fires_on_and_the_near_misses() {
        let engine = engine("no-tags", NO_TAGS);
        let rule = engine.rule("no-tags").unwrap();
        let entries = vec![
            fired(1, "a", "git tag v1", &[]),
            fired(2, "a", "git tag v1", &[]),
            fired(3, "a", "git status", &[]),
            fired(4, "b", "ls", &[]),
            fired(5, "b", "cd x && git tag -l", &[]),
            fired(6, "b", "git commit -m 'tag it'", &[]),
        ];
        let report = replay(&engine, rule, &entries);
        assert_eq!(report.calls, 6);
        assert_eq!(report.fired_total, 3);
        assert_eq!(
            report.fired[0],
            Hit {
                command: "git tag v1".into(),
                count: 2,
                verdict: "warn".into()
            }
        );
        let near: Vec<&str> = report
            .near_misses
            .iter()
            .map(|h| h.command.as_str())
            .collect();
        assert_eq!(near, ["git commit -m <arg>", "git status"]);
    }

    #[test]
    fn a_rule_that_fires_on_nothing_has_no_hits() {
        let engine = engine("no-tags", NO_TAGS);
        let report = replay(
            &engine,
            engine.rule("no-tags").unwrap(),
            &[fired(1, "a", "ls", &[])],
        );
        assert!(report.fired.is_empty() && report.near_misses.is_empty());
    }
}
