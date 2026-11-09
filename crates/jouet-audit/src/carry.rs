//! What each request re-read, charged to what put it in the context.
//!
//! A request reads the whole context, so content added once is paid for by
//! every later request until a compaction drops it. Between two requests the
//! context grows by what the first request's calls returned (and what the
//! model and the user wrote); that growth, times the requests that carried
//! it before the next compaction, is charged to those calls, split by the
//! bytes each returned. What a stretch between compactions starts with (the
//! fixed context and instructions, or a compaction's summary) is charged to
//! the start. Growth is measured from the requests' own token counts, so a
//! context that shrank without a compaction leaves a remainder, reported as
//! unattributed.

use crate::category::{Category, categorize};
use crate::event::Transcript;
use crate::redact::redact;
use serde::Serialize;
use std::collections::HashMap;

/// A drop in context this large, without a recorded compaction, is one.
pub(crate) const DROP: u64 = 20_000;
/// Files listed by what they kept costing.
const FILES: usize = 10;

#[derive(Debug, Clone, Default)]
pub(crate) struct CarryAcc {
    input: f64,
    start: f64,
    compactions: u64,
    categories: [f64; Category::ALL.len()],
    files: HashMap<String, f64>,
}

impl CarryAcc {
    /// `files` also charges each file a read or edit names: only when the
    /// report may show names, as with examples.
    pub(crate) fn add(&mut self, t: &Transcript, home: Option<&str>, files: bool) {
        let requests: Vec<(&crate::event::Request, u64)> = t
            .requests
            .iter()
            .filter_map(|r| Some((r, r.input_tokens?)))
            .collect();
        let mut start = 0;
        for i in 0..=requests.len() {
            let ends = i == requests.len()
                || i > start
                    && (requests[i].0.compacted || requests[i].1 + DROP < requests[i - 1].1);
            if !ends {
                continue;
            }
            if i > start {
                if start > 0 {
                    self.compactions += 1;
                }
                self.segment(t, &requests[start..i], home, files);
            }
            start = i;
        }
    }

    fn segment(
        &mut self,
        t: &Transcript,
        requests: &[(&crate::event::Request, u64)],
        home: Option<&str>,
        files: bool,
    ) {
        let n = requests.len();
        self.input += requests.iter().map(|(_, c)| *c as f64).sum::<f64>();
        self.start += requests[0].1 as f64 * n as f64;
        for (i, pair) in requests.windows(2).enumerate() {
            let added = pair[1].1.saturating_sub(pair[0].1) as f64;
            // Every request from the next one to the segment's end read it.
            let carried = added * (n - 1 - i) as f64;
            if carried == 0.0 {
                continue;
            }
            let calls = t.calls.get(pair[0].0.calls.clone()).unwrap_or_default();
            if calls.is_empty() {
                self.categories[Category::Reply.index()] += carried;
                continue;
            }
            let weight = |c: &crate::event::Call| c.output_bytes as f64 + 1.0;
            let total: f64 = calls.iter().map(weight).sum();
            for call in calls {
                let part = carried * weight(call) / total;
                self.categories[categorize(call).index()] += part;
                if let Some(target) = call.target.as_ref().filter(|_| files) {
                    *self
                        .files
                        .entry(file(target, t.cwd.as_deref(), home))
                        .or_default() += part;
                }
            }
        }
    }

    pub(crate) fn merge(&mut self, theirs: Self) {
        self.input += theirs.input;
        self.start += theirs.start;
        self.compactions += theirs.compactions;
        for (o, t) in self.categories.iter_mut().zip(theirs.categories) {
            *o += t;
        }
        for (f, n) in theirs.files {
            *self.files.entry(f).or_default() += n;
        }
    }

    pub(crate) fn report(&self) -> Option<Carried> {
        if self.input <= 0.0 {
            return None;
        }
        let share = |n: f64| (n / self.input * 10_000.0).round() / 10_000.0;
        let mut categories: Vec<CarriedCategory> = Category::ALL
            .into_iter()
            .filter(|c| self.categories[c.index()] >= 0.5)
            .map(|category| {
                let n = self.categories[category.index()];
                CarriedCategory {
                    category,
                    input_tokens: n.round() as u64,
                    share: share(n),
                }
            })
            .collect();
        categories.sort_by(|a, b| {
            b.input_tokens
                .cmp(&a.input_tokens)
                .then(a.category.cmp(&b.category))
        });
        let mut files: Vec<(&String, &f64)> = self.files.iter().collect();
        files.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
        let attributed: f64 = self.start + self.categories.iter().sum::<f64>();
        Some(Carried {
            input_tokens: self.input.round() as u64,
            start_tokens: self.start.round() as u64,
            start_share: share(self.start),
            compactions: self.compactions,
            unattributed_tokens: (self.input - attributed).round() as i64,
            categories,
            files: files
                .into_iter()
                .take(FILES)
                .map(|(f, n)| CarriedFile {
                    file: f.clone(),
                    input_tokens: n.round() as u64,
                    share: share(*n),
                })
                .collect(),
        })
    }
}

/// A target as shown: relative to the session's directory, else with the
/// home directory written `~`, and redacted.
fn file(target: &str, cwd: Option<&str>, home: Option<&str>) -> String {
    let under = |dir: &str| {
        let dir = dir.trim_end_matches('/');
        (!dir.is_empty())
            .then(|| target.strip_prefix(dir)?.strip_prefix('/'))
            .flatten()
    };
    let shown = match (cwd.and_then(under), home.and_then(under)) {
        (Some(rel), _) => rel.to_string(),
        (None, Some(rel)) => format!("~/{rel}"),
        (None, None) => target.to_string(),
    };
    redact(&shown)
}

/// Each request's input, charged to what put it in the context.
#[derive(Debug, Clone, Serialize)]
pub struct Carried {
    /// The input of the requests counted: those with token counts.
    pub input_tokens: u64,
    /// What each stretch between compactions started with, times its
    /// requests: the fixed context and instructions, or a summary.
    pub start_tokens: u64,
    pub start_share: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub compactions: u64,
    /// Input left over where a context shrank without a compaction:
    /// negative when more was charged than read.
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub unattributed_tokens: i64,
    /// Most first.
    pub categories: Vec<CarriedCategory>,
    /// The files whose reads and edits kept costing most.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<CarriedFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarriedCategory {
    pub category: Category,
    pub input_tokens: u64,
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarriedFile {
    pub file: String,
    pub input_tokens: u64,
    pub share: f64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Call, Request, Tool};

    fn request(input: u64, calls: std::ops::Range<usize>, compacted: bool) -> Request {
        Request {
            input_tokens: Some(input),
            calls,
            compacted,
            ..Default::default()
        }
    }

    #[test]
    fn growth_is_charged_to_the_call_that_added_it_until_a_compaction() {
        let mut read = Call::new(Tool::Read);
        read.target = Some("/work/app/src/big.rs".into());
        read.output_bytes = 30_000;
        let t = Transcript {
            session: "s".into(),
            cwd: Some("/work/app".into()),
            calls: vec![read, Call::shell("git status")],
            requests: vec![
                // Reads a file: the context grows by 10k.
                request(30_000, 0..1, false),
                // Runs git: grows by 1k.
                request(40_000, 1..2, false),
                request(41_000, 2..2, false),
                request(41_000, 2..2, false),
                // A compaction: the file is no longer carried.
                request(20_000, 2..2, true),
                request(20_000, 2..2, false),
            ],
            ..Default::default()
        };
        let mut acc = CarryAcc::default();
        acc.add(&t, Some("/home/me"), true);
        let r = acc.report().unwrap();
        assert_eq!(r.input_tokens, 192_000);
        assert_eq!(r.compactions, 1);
        // 30k for four requests, then 20k for two.
        assert_eq!(r.start_tokens, 160_000);
        assert_eq!(r.categories[0].category, Category::ReadSearch);
        // 10k, read by three later requests.
        assert_eq!(r.categories[0].input_tokens, 30_000);
        assert_eq!(r.categories[1].category, Category::Git);
        assert_eq!(r.categories[1].input_tokens, 2_000);
        assert_eq!(r.unattributed_tokens, 0);
        assert_eq!(r.files[0].file, "src/big.rs");
        assert_eq!(r.files[0].input_tokens, 30_000);
    }

    #[test]
    fn a_large_drop_without_a_record_is_a_compaction() {
        let t = Transcript {
            requests: vec![
                request(100_000, 0..0, false),
                request(150_000, 0..0, false),
                request(40_000, 0..0, false),
            ],
            ..Default::default()
        };
        let mut acc = CarryAcc::default();
        acc.add(&t, None, false);
        let r = acc.report().unwrap();
        assert_eq!(r.compactions, 1);
        assert_eq!(r.categories[0].category, Category::Reply);
        assert_eq!(r.categories[0].input_tokens, 50_000);
    }

    #[test]
    fn files_are_shown_relative_or_from_home() {
        assert_eq!(file("/work/app/a.rs", Some("/work/app"), None), "a.rs");
        assert_eq!(
            file("/home/me/x/b.rs", Some("/work/app"), Some("/home/me")),
            "~/x/b.rs"
        );
        assert_eq!(
            file("/work/apple/c.rs", Some("/work/app"), None),
            "/work/apple/c.rs"
        );
    }
}
