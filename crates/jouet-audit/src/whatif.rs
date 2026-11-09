//! What the same sessions would have read with fewer definitions, or with
//! the conversation compacted at a smaller window.
//!
//! Each request's context comes from its own token count. Denying a
//! definition takes its tokens off every request of a transcript that had
//! it, which replays exactly up to the estimate of its size. A window is
//! simulated: when the context would pass the window less a reserve, the
//! conversation is compacted to what the transcript started with plus a
//! summary ([`POST`], as measured on real compactions), charging one read of
//! the whole context for the summary and one cache write of what is left.
//! Where compaction fires inside the window is not known, so the window is
//! replayed with no reserve and with [`RESERVE`], and reported as a range.
//! It assumes the same requests follow a compaction, which real sessions do
//! not quite: they re-read some of what was dropped.
//!
//! Prices weight an uncached token 1, a cache write 1.25 and a cache read
//! 0.1, and each request keeps its own mix.

use crate::carry::DROP;
use crate::event::Transcript;
use crate::fixed::{item, tokens};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Context left after a compaction, above what the transcript started with.
const POST: f64 = 19_000.0;
/// How far below the window compaction may fire.
const RESERVE: u64 = 33_000;
/// The smallest context a request is replayed with.
const FLOOR: f64 = 1_000.0;
const CACHE_READ: f64 = 0.1;
const CACHE_WRITE: f64 = 1.25;

/// What to replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WhatIf {
    /// Compact at this many tokens.
    pub window: Option<u64>,
    /// Definitions to remove, by name as the fixed-context report shows
    /// them: a tool, an MCP server, a plugin or a skill.
    pub deny: Vec<String>,
}

impl WhatIf {
    /// `window=200k,deny=Workflow,Artifact`: a name after `deny=` with no
    /// key of its own is denied too.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut w = Self::default();
        let mut denying = false;
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some(("window", n)) => {
                    w.window = Some(size(n).ok_or_else(|| {
                        format!("window=`{n}` is not a size; write it as 200k, 1m or 200000")
                    })?);
                    denying = false;
                }
                Some(("deny", name)) if !name.is_empty() => {
                    w.deny.push(name.to_string());
                    denying = true;
                }
                None if denying => w.deny.push(part.to_string()),
                _ => {
                    return Err(format!("`{part}` is not window=SIZE or deny=NAME"));
                }
            }
        }
        if w.window.is_none() && w.deny.is_empty() {
            return Err("give window=SIZE, deny=NAME or both".into());
        }
        if w.window.is_some_and(|n| n < 10_000) {
            return Err("a window under 10k tokens cannot hold a request".into());
        }
        Ok(w)
    }
}

fn size(s: &str) -> Option<u64> {
    let lower = s.to_ascii_lowercase();
    let (digits, scale) = match lower.strip_suffix('k') {
        Some(d) => (d, 1_000.0),
        None => match lower.strip_suffix('m') {
            Some(d) => (d, 1_000_000.0),
            None => (lower.as_str(), 1.0),
        },
    };
    let n: f64 = digits.parse().ok()?;
    (n.is_finite() && n > 0.0).then(|| (n * scale).round() as u64)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WhatIfAcc {
    requests: u64,
    tokens: f64,
    price: f64,
    /// Removed, by name: tokens and price.
    denied: BTreeMap<String, (f64, f64)>,
    /// With no reserve and with [`RESERVE`]: tokens, price, compactions.
    sims: [(f64, f64, u64); 2],
}

impl WhatIfAcc {
    pub(crate) fn add(&mut self, spec: &WhatIf, t: &Transcript) {
        // Context, and price per token of context.
        let requests: Vec<(f64, f64, bool)> = t
            .requests
            .iter()
            .filter_map(|r| {
                let c = r.input_tokens? as f64;
                let read = r.cached_input_tokens.unwrap_or(0) as f64;
                let write = r.cache_write_tokens.unwrap_or(0) as f64;
                let rest = (c - read - write).max(0.0);
                let price = rest + write * CACHE_WRITE + read * CACHE_READ;
                (c > 0.0).then_some((c, price / c, r.compacted))
            })
            .collect();
        if requests.is_empty() {
            return;
        }
        let per_token: f64 = requests.iter().map(|r| r.1).sum();
        self.requests += requests.len() as u64;
        self.tokens += requests.iter().map(|r| r.0).sum::<f64>();
        self.price += requests.iter().map(|r| r.0 * r.1).sum::<f64>();
        let mut d = 0.0;
        if let Some(defs) = &t.definitions {
            let names: BTreeSet<&str> = spec.deny.iter().map(String::as_str).collect();
            for def in &defs.items {
                let (_, name) = item(def);
                if let Some(n) = names.get(name.as_str()) {
                    let size = tokens(def);
                    d += size;
                    let e = self.denied.entry((*n).to_string()).or_default();
                    e.0 += size * requests.len() as f64;
                    e.1 += size * per_token;
                }
            }
        }
        for (slot, reserve) in [0, RESERVE].into_iter().enumerate() {
            let start = t.start_tokens.map_or(requests[0].0, |n| n as f64);
            let (tokens, price, compactions) = replay(&requests, start, d, spec.window, reserve);
            let s = &mut self.sims[slot];
            s.0 += tokens;
            s.1 += price;
            s.2 += compactions;
        }
    }

    pub(crate) fn merge(&mut self, theirs: Self) {
        self.requests += theirs.requests;
        self.tokens += theirs.tokens;
        self.price += theirs.price;
        for (name, (t, p)) in theirs.denied {
            let e = self.denied.entry(name).or_default();
            e.0 += t;
            e.1 += p;
        }
        for (o, t) in self.sims.iter_mut().zip(theirs.sims) {
            o.0 += t.0;
            o.1 += t.1;
            o.2 += t.2;
        }
    }

    pub(crate) fn report(&self, spec: &WhatIf) -> Option<WhatIfReport> {
        if self.requests == 0 {
            return None;
        }
        let ratio = |part: f64, whole: f64| ((1.0 - part / whole) * 10_000.0).round() / 10_000.0;
        let scenario = |(tokens, price, compactions): (f64, f64, u64), reserve| Scenario {
            reserve,
            input_tokens: tokens.round() as u64,
            weighted_input_tokens: price.round() as u64,
            saved: ratio(tokens, self.tokens),
            weighted_saved: ratio(price, self.price),
            compactions,
        };
        let scenarios = if spec.window.is_some() {
            vec![scenario(self.sims[0], 0), scenario(self.sims[1], RESERVE)]
        } else {
            vec![scenario(self.sims[0], 0)]
        };
        Some(WhatIfReport {
            window: spec.window,
            deny: spec.deny.clone(),
            not_found: spec
                .deny
                .iter()
                .filter(|n| !self.denied.contains_key(*n))
                .cloned()
                .collect(),
            requests: self.requests,
            input_tokens: self.tokens.round() as u64,
            weighted_input_tokens: self.price.round() as u64,
            denied: self
                .denied
                .iter()
                .map(|(name, (t, p))| Denied {
                    name: name.clone(),
                    input_tokens: t.round() as u64,
                    weighted_input_tokens: p.round() as u64,
                })
                .collect(),
            scenarios,
        })
    }
}

/// One transcript replayed from `start`, the context it began with: its
/// input tokens, their price, and the compactions the window added.
fn replay(
    requests: &[(f64, f64, bool)],
    start: f64,
    d: f64,
    window: Option<u64>,
    reserve: u64,
) -> (f64, f64, u64) {
    let fire = window.map(|w| w.saturating_sub(reserve) as f64);
    let (mut tokens, mut price, mut compactions) = (0.0, 0.0, 0);
    // The context as it would have been, before the denied definitions.
    let mut s = requests[0].0;
    let mut previous = s;
    for (i, &(c, per_token, compacted)) in requests.iter().enumerate() {
        if i > 0 {
            s = if compacted || c + (DROP as f64) < previous {
                s.min(c)
            } else {
                s + (c - previous)
            };
        }
        previous = c;
        // A compaction keeps what the transcript started with and a summary,
        // and never leaves more than it found. The denied definitions are
        // gone from the context it measures.
        let after = (start + POST).min(s);
        if let Some(fire) = fire
            && s - d > fire
            && after < s
        {
            compactions += 1;
            // The summary reads the whole context, cached; what is left is
            // written to the cache again.
            let read = (s - d).max(FLOOR);
            tokens += read;
            price += read * CACHE_READ;
            s = after;
            price += (s - d).max(FLOOR) * CACHE_WRITE;
        }
        let x = (s - d).max(FLOOR);
        tokens += x;
        price += x * per_token;
    }
    (tokens, price, compactions)
}

/// The replay's result for one harness.
#[derive(Debug, Clone, Serialize)]
pub struct WhatIfReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    /// Denied names no transcript had.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<String>,
    /// The requests replayed (those with token counts), their input, and
    /// its price-weighted sum.
    pub requests: u64,
    pub input_tokens: u64,
    pub weighted_input_tokens: u64,
    /// What each denial takes off, exactly, before any window.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub denied: Vec<Denied>,
    /// With a window, two: compaction firing at the window, and at the
    /// window less 33k. Without one, the denials alone.
    pub scenarios: Vec<Scenario>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Denied {
    pub name: String,
    pub input_tokens: u64,
    pub weighted_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scenario {
    /// How far below the window compaction fired.
    pub reserve: u64,
    /// The input replayed, and its price-weighted sum.
    pub input_tokens: u64,
    pub weighted_input_tokens: u64,
    /// The fraction saved, 0–1; negative when the replay reads more.
    pub saved: f64,
    pub weighted_saved: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub compactions: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Definition, DefinitionKind, Definitions, Request};

    #[test]
    fn specs_parse_windows_and_lists_of_denials() {
        assert_eq!(
            WhatIf::parse("window=200k,deny=Workflow,Artifact").unwrap(),
            WhatIf {
                window: Some(200_000),
                deny: vec!["Workflow".into(), "Artifact".into()],
            }
        );
        assert_eq!(WhatIf::parse("window=1m").unwrap().window, Some(1_000_000));
        assert_eq!(
            WhatIf::parse("window=160000").unwrap().window,
            Some(160_000)
        );
        assert!(WhatIf::parse("Workflow").is_err());
        assert!(WhatIf::parse("window=big").is_err());
        assert!(WhatIf::parse("window=5k").is_err());
        assert!(WhatIf::parse("").is_err());
    }

    fn transcript(contexts: &[u64]) -> Transcript {
        Transcript {
            requests: contexts
                .iter()
                .map(|&c| Request {
                    input_tokens: Some(c),
                    // All read from the cache.
                    cached_input_tokens: Some(c),
                    ..Default::default()
                })
                .collect(),
            definitions: Some(Definitions {
                at: None,
                items: vec![Definition {
                    kind: DefinitionKind::Tool,
                    name: "Workflow".into(),
                    bytes: 9_200,
                }],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_denial_takes_its_tokens_off_every_request() {
        let spec = WhatIf::parse("deny=Workflow,Nope").unwrap();
        let mut acc = WhatIfAcc::default();
        acc.add(&spec, &transcript(&[50_000, 60_000, 70_000]));
        let r = acc.report(&spec).unwrap();
        assert_eq!(r.input_tokens, 180_000);
        assert_eq!(r.denied[0].input_tokens, 6_000);
        assert_eq!(r.denied[0].weighted_input_tokens, 600);
        assert_eq!(r.scenarios.len(), 1);
        assert_eq!(r.scenarios[0].input_tokens, 174_000);
        assert_eq!(r.not_found, ["Nope"]);
    }

    #[test]
    fn a_window_compacts_and_the_reserve_gives_a_range() {
        let spec = WhatIf::parse("window=100k").unwrap();
        let mut acc = WhatIfAcc::default();
        // Starts at 20k and grows 20k a request to 180k.
        let contexts: Vec<u64> = (1..=9).map(|n| n * 20_000).collect();
        acc.add(&spec, &transcript(&contexts));
        let r = acc.report(&spec).unwrap();
        let (late, early) = (&r.scenarios[0], &r.scenarios[1]);
        assert_eq!((late.reserve, early.reserve), (0, RESERVE));
        // At the window: 120k compacts to 20k + 19k, which grows to 99k.
        // 33k below it, compaction fires three times.
        assert_eq!((late.compactions, early.compactions), (1, 3));
        assert!(late.saved > 0.0 && early.saved > late.saved, "{r:?}");
        // Summaries and cache writes cost something.
        assert!(late.weighted_saved < late.saved);
    }

    #[test]
    fn a_window_compacts_to_where_the_transcript_started_not_where_since_cut_it() {
        let spec = WhatIf::parse("window=100k").unwrap();
        let mut t = transcript(&[150_000, 160_000]);
        // `--since` dropped the requests before 150k; it started at 20k.
        t.start_tokens = Some(20_000);
        let mut acc = WhatIfAcc::default();
        acc.add(&spec, &t);
        let late = &acc.report(&spec).unwrap().scenarios[0];
        // One compaction to 39k, which then grows by 10k.
        assert_eq!(late.compactions, 1);
        assert_eq!(late.input_tokens, 150_000 + 39_000 + 49_000);
    }

    #[test]
    fn denied_definitions_leave_room_before_the_window() {
        // 4.6k characters a token: 92k bytes is 20k tokens.
        let mut t = transcript(&[50_000, 110_000, 115_000]);
        t.definitions.as_mut().unwrap().items[0].bytes = 92_000;
        let mut acc = WhatIfAcc::default();
        let spec = WhatIf::parse("window=100k,deny=Workflow").unwrap();
        acc.add(&spec, &t);
        let late = &acc.report(&spec).unwrap().scenarios[0];
        assert_eq!(late.compactions, 0);
        assert_eq!(late.input_tokens, 275_000 - 60_000);
    }

    #[test]
    fn a_real_compaction_resets_the_replay() {
        let spec = WhatIf::parse("window=200k").unwrap();
        let mut acc = WhatIfAcc::default();
        acc.add(&spec, &transcript(&[50_000, 150_000, 30_000, 60_000]));
        let r = acc.report(&spec).unwrap();
        assert_eq!(r.scenarios[0].compactions, 0);
        assert_eq!(r.scenarios[0].input_tokens, r.input_tokens);
    }
}
