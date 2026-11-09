//! The fixed context: the definitions a harness sends with every request
//! (tools, the skill listing, MCP servers' instructions), what they cost
//! across the requests that carried them, and whether anything used them.
//!
//! Sizes come from each transcript's first snapshot of its definitions and
//! are converted to tokens at a fixed rate, so they are estimates: tool JSON
//! at 4.6 characters a token (Claude Code's Artifact tool, 58k characters,
//! measured 12.8k tokens), prose at 3.6.

use crate::event::{Call, Definition, DefinitionKind, Transcript};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

const TOOL_CHARS_PER_TOKEN: f64 = 4.6;
const PROSE_CHARS_PER_TOKEN: f64 = 3.6;
/// Items whose mean per session grew by less than this, in tokens, between
/// two periods are not named as growth.
const GROWTH: f64 = 100.0;
/// How many items a period names as what grew, and as what shrank.
const GREW: usize = 5;
const DAY_MS: i64 = 86_400_000;

/// What a definition belongs to, as a thing a setting can remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A built-in tool, removed one by one.
    Tool,
    /// An MCP server: its tools and its instructions.
    McpServer,
    /// A plugin's skills, enabled or disabled together.
    Plugin,
    /// A skill of no plugin.
    Skill,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::McpServer => "MCP server",
            Self::Plugin => "plugin",
            Self::Skill => "skill",
        }
    }
}

type Key = (ItemKind, String);

pub(crate) fn item(d: &Definition) -> Key {
    match d.kind {
        DefinitionKind::Tool => match mcp_server(&d.name) {
            Some(server) => (ItemKind::McpServer, server.to_string()),
            None => (ItemKind::Tool, d.name.clone()),
        },
        DefinitionKind::McpInstructions => (ItemKind::McpServer, d.name.clone()),
        DefinitionKind::Skill => skill(&d.name),
    }
}

fn mcp_server(tool: &str) -> Option<&str> {
    Some(tool.strip_prefix("mcp__")?.split_once("__")?.0)
}

fn skill(name: &str) -> Key {
    match name.split_once(':') {
        Some((plugin, _)) => (ItemKind::Plugin, plugin.to_string()),
        None => (ItemKind::Skill, name.to_string()),
    }
}

pub(crate) fn tokens(d: &Definition) -> f64 {
    let rate = match d.kind {
        DefinitionKind::Tool => TOOL_CHARS_PER_TOKEN,
        DefinitionKind::Skill | DefinitionKind::McpInstructions => PROSE_CHARS_PER_TOKEN,
    };
    d.bytes as f64 / rate
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FixedAcc {
    transcripts: u64,
    requests: u64,
    /// Tokens of definitions times the requests that carried them.
    tokens: f64,
    items: HashMap<Key, ItemAcc>,
    calls: HashMap<Key, u64>,
    /// By the first day (a Monday) of the week the snapshot was taken.
    periods: BTreeMap<i64, PeriodAcc>,
}

#[derive(Debug, Clone, Default)]
struct ItemAcc {
    transcripts: u64,
    requests: u64,
    tokens: f64,
    /// The item's tokens summed over the transcripts it was in, for a mean.
    per_transcript: f64,
}

#[derive(Debug, Clone, Default)]
struct PeriodAcc {
    transcripts: u64,
    requests: u64,
    /// Each transcript's definitions, summed, for a mean.
    per_transcript: f64,
    items: HashMap<Key, f64>,
}

impl FixedAcc {
    /// Counts a transcript's definitions against the requests it kept.
    pub(crate) fn add(&mut self, t: &Transcript) {
        let Some(definitions) = &t.definitions else {
            return;
        };
        let requests = t.requests.len() as u64;
        if requests == 0 || definitions.items.is_empty() {
            return;
        }
        let mut by_item: HashMap<Key, f64> = HashMap::new();
        for d in &definitions.items {
            *by_item.entry(item(d)).or_default() += tokens(d);
        }
        let total: f64 = by_item.values().sum();
        self.transcripts += 1;
        self.requests += requests;
        self.tokens += total * requests as f64;
        let at = definitions
            .at
            .or_else(|| t.requests.iter().find_map(|r| r.at));
        let period = at.map(|ms| {
            let day = ms.div_euclid(DAY_MS);
            // 1970-01-01 was a Thursday.
            day - (day + 3).rem_euclid(7)
        });
        for (key, tokens) in &by_item {
            let i = self.items.entry(key.clone()).or_default();
            i.transcripts += 1;
            i.requests += requests;
            i.tokens += tokens * requests as f64;
            i.per_transcript += tokens;
        }
        if let Some(period) = period {
            let p = self.periods.entry(period).or_default();
            p.transcripts += 1;
            p.requests += requests;
            p.per_transcript += total;
            for (key, tokens) in by_item {
                *p.items.entry(key).or_default() += tokens;
            }
        }
    }

    /// Counts a call against the definition it used: its tool, its MCP
    /// server, and for a `Skill` call the skill or plugin.
    pub(crate) fn count(&mut self, call: &Call) {
        let Some(name) = call.name.as_deref() else {
            return;
        };
        let key = match mcp_server(name) {
            Some(server) => (ItemKind::McpServer, server.to_string()),
            None => (ItemKind::Tool, name.to_string()),
        };
        *self.calls.entry(key).or_default() += 1;
        if let Some(s) = &call.skill {
            *self.calls.entry(skill(s)).or_default() += 1;
        }
    }

    pub(crate) fn merge(&mut self, theirs: Self) {
        self.transcripts += theirs.transcripts;
        self.requests += theirs.requests;
        self.tokens += theirs.tokens;
        for (key, t) in theirs.items {
            let o = self.items.entry(key).or_default();
            o.transcripts += t.transcripts;
            o.requests += t.requests;
            o.tokens += t.tokens;
            o.per_transcript += t.per_transcript;
        }
        for (key, n) in theirs.calls {
            *self.calls.entry(key).or_default() += n;
        }
        for (period, t) in theirs.periods {
            let o = self.periods.entry(period).or_default();
            o.transcripts += t.transcripts;
            o.requests += t.requests;
            o.per_transcript += t.per_transcript;
            for (key, tokens) in t.items {
                *o.items.entry(key).or_default() += tokens;
            }
        }
    }

    /// The report, or `None` when no transcript recorded its definitions.
    pub(crate) fn report(&self, input_tokens: u64) -> Option<FixedContext> {
        if self.transcripts == 0 {
            return None;
        }
        let mut items: Vec<FixedItem> = self
            .items
            .iter()
            .map(|((kind, name), i)| {
                let calls = self.calls.get(&(*kind, name.clone())).copied().unwrap_or(0);
                FixedItem {
                    kind: *kind,
                    name: name.clone(),
                    tokens_per_request: (i.per_transcript / i.transcripts as f64).round() as u64,
                    transcripts: i.transcripts,
                    requests: i.requests,
                    input_tokens: i.tokens.round() as u64,
                    calls,
                    remove: (calls == 0).then(|| remove(*kind, name)),
                }
            })
            .collect();
        items.sort_by(|a, b| {
            b.input_tokens
                .cmp(&a.input_tokens)
                .then(a.kind.cmp(&b.kind))
                .then(a.name.cmp(&b.name))
        });
        let mut periods = Vec::new();
        let mut previous: Option<&PeriodAcc> = None;
        for (&day, p) in &self.periods {
            let mean = |p: &PeriodAcc, key: &Key| {
                p.items.get(key).copied().unwrap_or(0.0) / p.transcripts as f64
            };
            let mut changes: Vec<(f64, Growth)> = previous
                .map(|before| {
                    let keys: std::collections::BTreeSet<&Key> =
                        p.items.keys().chain(before.items.keys()).collect();
                    keys.into_iter()
                        .filter_map(|key| {
                            let (b, a) = (mean(before, key), mean(p, key));
                            ((a - b).abs() >= GROWTH).then(|| {
                                let change = Growth {
                                    kind: key.0,
                                    name: key.1.clone(),
                                    before: b.round() as u64,
                                    after: a.round() as u64,
                                };
                                (a - b, change)
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            changes.sort_by(|a, b| {
                b.0.abs()
                    .total_cmp(&a.0.abs())
                    .then(a.1.name.cmp(&b.1.name))
            });
            let (mut grew, mut shrank) = (Vec::new(), Vec::new());
            for (delta, change) in changes {
                let list = if delta > 0.0 { &mut grew } else { &mut shrank };
                if list.len() < GREW {
                    list.push(change);
                }
            }
            periods.push(FixedPeriod {
                start: crate::time::date(day),
                transcripts: p.transcripts,
                requests: p.requests,
                mean_tokens: (p.per_transcript / p.transcripts as f64).round() as u64,
                grew,
                shrank,
            });
            previous = Some(p);
        }
        Some(FixedContext {
            transcripts: self.transcripts,
            requests: self.requests,
            mean_tokens: (self.items.values().map(|i| i.per_transcript).sum::<f64>()
                / self.transcripts as f64)
                .round() as u64,
            input_tokens: self.tokens.round() as u64,
            share: if input_tokens > 0 {
                (self.tokens / input_tokens as f64 * 10_000.0).round() / 10_000.0
            } else {
                0.0
            },
            items,
            periods,
        })
    }
}

/// The setting that removes an item nothing called.
fn remove(kind: ItemKind, name: &str) -> String {
    match kind {
        ItemKind::Tool if name == "Artifact" => "enableArtifact: false".into(),
        ItemKind::Tool => format!("permissions.deny: [\"{name}\"]"),
        ItemKind::McpServer if name.starts_with("ccd_") => {
            "the desktop app provides it; no setting is known to remove it".into()
        }
        ItemKind::McpServer => "disconnect the MCP server".into(),
        ItemKind::Plugin => format!("enabledPlugins: {{\"{name}@<marketplace>\": false}}"),
        ItemKind::Skill => {
            "remove it from its skills directory, or disableBundledSkills: true if Claude Code bundles it".into()
        }
    }
}

/// The definitions sent with every request, from each transcript's first
/// snapshot of them.
#[derive(Debug, Clone, Serialize)]
pub struct FixedContext {
    /// Transcripts that recorded their definitions.
    pub transcripts: u64,
    /// The requests those transcripts made.
    pub requests: u64,
    /// The definitions of a transcript, in estimated tokens, on average:
    /// what each of its requests carried.
    pub mean_tokens: u64,
    /// Every request's definitions, summed.
    pub input_tokens: u64,
    /// Of the harness's input tokens, 0–1.
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub share: f64,
    /// Most input tokens first.
    pub items: Vec<FixedItem>,
    /// By week, from the Monday (UTC) each starts on.
    pub periods: Vec<FixedPeriod>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixedItem {
    pub kind: ItemKind,
    pub name: String,
    /// Its estimated tokens, the mean over the transcripts it was in.
    pub tokens_per_request: u64,
    /// The transcripts it was in, and their requests.
    pub transcripts: u64,
    pub requests: u64,
    /// Its tokens times those requests.
    pub input_tokens: u64,
    /// Calls to it (a skill's through the `Skill` tool) in every transcript.
    #[serde(skip_serializing_if = "is_zero")]
    pub calls: u64,
    /// For an item nothing called, the setting that removes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixedPeriod {
    /// The Monday the week starts on, `YYYY-MM-DD`.
    pub start: String,
    pub transcripts: u64,
    pub requests: u64,
    /// A transcript's definitions, on average, in estimated tokens.
    pub mean_tokens: u64,
    /// The items that grew most since the period before, by their mean per
    /// transcript.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub grew: Vec<Growth>,
    /// The items that shrank most, or were gone.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shrank: Vec<Growth>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Growth {
    pub kind: ItemKind,
    pub name: String,
    /// Its mean tokens per transcript in the period before, and in this one.
    #[serde(skip_serializing_if = "is_zero")]
    pub before: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub after: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Definitions, Request, Tool};

    fn def(kind: DefinitionKind, name: &str, bytes: u64) -> Definition {
        Definition {
            kind,
            name: name.into(),
            bytes,
        }
    }

    fn transcript(at: i64, requests: usize, items: Vec<Definition>) -> Transcript {
        Transcript {
            session: format!("s{at}"),
            requests: vec![Request::default(); requests],
            definitions: Some(Definitions {
                at: Some(at),
                items,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn items_cost_their_tokens_on_every_request_and_unused_ones_name_a_setting() {
        let mut acc = FixedAcc::default();
        acc.add(&transcript(
            0,
            10,
            vec![
                def(DefinitionKind::Tool, "Bash", 4600),
                def(DefinitionKind::Tool, "Artifact", 46_000),
                def(DefinitionKind::Tool, "mcp__ccd_turn__update_plan", 460),
                def(DefinitionKind::McpInstructions, "computer-use", 3600),
                def(DefinitionKind::Skill, "anthropic-skills:docx", 360),
                def(DefinitionKind::Skill, "anthropic-skills:pdf", 360),
                def(DefinitionKind::Skill, "pr-description", 36),
            ],
        ));
        let mut bash = Call::new(Tool::Shell);
        bash.name = Some("Bash".into());
        let mut plan = Call::new(Tool::Other);
        plan.name = Some("mcp__ccd_turn__update_plan".into());
        let mut skill_call = Call::new(Tool::Other);
        skill_call.name = Some("Skill".into());
        skill_call.skill = Some("pr-description".into());
        for call in [&bash, &bash, &plan, &skill_call] {
            acc.count(call);
        }
        let report = acc.report(1_000_000).unwrap();
        assert_eq!(report.transcripts, 1);
        assert_eq!(report.mean_tokens, 1000 + 10_000 + 100 + 1000 + 200 + 10);
        assert_eq!(report.input_tokens, 123_100);
        assert_eq!(report.share, 0.1231);
        let row = |name: &str| report.items.iter().find(|i| i.name == name).unwrap();
        assert_eq!(report.items[0].name, "Artifact");
        assert_eq!(
            row("Artifact").remove.as_deref(),
            Some("enableArtifact: false")
        );
        assert_eq!(row("Bash").calls, 2);
        assert_eq!(row("Bash").input_tokens, 10_000);
        assert!(row("Bash").remove.is_none());
        assert_eq!(row("ccd_turn").kind, ItemKind::McpServer);
        assert_eq!(row("ccd_turn").calls, 1);
        assert_eq!(
            row("computer-use").remove.as_deref(),
            Some("disconnect the MCP server")
        );
        assert_eq!(row("anthropic-skills").kind, ItemKind::Plugin);
        assert_eq!(row("anthropic-skills").tokens_per_request, 200);
        assert_eq!(row("pr-description").calls, 1);
    }

    #[test]
    fn periods_are_weeks_and_name_what_grew() {
        let monday = crate::time::parse_timestamp("2026-11-02T00:00:00Z").unwrap();
        let week = 7 * DAY_MS;
        let mut a = FixedAcc::default();
        let mut b = FixedAcc::default();
        a.add(&transcript(
            monday + DAY_MS,
            2,
            vec![def(DefinitionKind::Tool, "Bash", 4600)],
        ));
        b.add(&transcript(
            monday + week + 3 * DAY_MS,
            1,
            vec![
                def(DefinitionKind::Tool, "Bash", 4600),
                def(DefinitionKind::Tool, "Workflow", 9200),
            ],
        ));
        // Not a week of its own: the Sunday before belongs to the week
        // before.
        b.add(&transcript(
            monday - 1,
            1,
            vec![def(DefinitionKind::Tool, "Bash", 4600)],
        ));
        a.merge(b);
        let report = a.report(0).unwrap();
        let starts: Vec<&str> = report.periods.iter().map(|p| p.start.as_str()).collect();
        assert_eq!(starts, ["2026-10-26", "2026-11-02", "2026-11-09"]);
        assert_eq!(report.periods[2].mean_tokens, 3000);
        let grew = &report.periods[2].grew;
        assert_eq!(grew.len(), 1);
        assert_eq!(
            (grew[0].name.as_str(), grew[0].before, grew[0].after),
            ("Workflow", 0, 2000)
        );
        assert!(report.periods[1].grew.is_empty());
        assert!(report.periods[1].shrank.is_empty());

        let mut later = FixedAcc::default();
        later.add(&transcript(
            monday + 2 * week,
            1,
            vec![def(DefinitionKind::Tool, "Bash", 4600)],
        ));
        a.merge(later);
        let report = a.report(0).unwrap();
        let shrank = &report.periods[3].shrank;
        assert_eq!(
            (shrank[0].name.as_str(), shrank[0].before, shrank[0].after),
            ("Workflow", 2000, 0)
        );
    }

    #[test]
    fn transcripts_without_definitions_or_requests_are_left_out() {
        let mut acc = FixedAcc::default();
        acc.add(&Transcript::default());
        acc.add(&transcript(
            0,
            0,
            vec![def(DefinitionKind::Tool, "Bash", 1)],
        ));
        assert!(acc.report(10).is_none());
    }
}
