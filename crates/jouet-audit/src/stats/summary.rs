//! The one line a session's stop hook can print about what jouet saved.

use super::{Estimate, Kind, Measured, tokens_text};

/// How the summary line is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SummaryStyle {
    /// A line with a little personality, grounded in the session's numbers.
    #[default]
    Quips,
    /// The same fact, stated plainly.
    Plain,
    /// No line.
    Off,
}

impl SummaryStyle {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "quips" => Some(Self::Quips),
            "plain" => Some(Self::Plain),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Below this many estimated tokens, waits are not worth a line.
const MIN_WAIT_TOKENS: u64 = 50_000;

/// One fact worth telling, in each voice.
struct Candidate {
    quips: Vec<String>,
    plain: String,
}

/// One line about what jouet saved, or `None` when nothing is worth saying
/// or the style is [`SummaryStyle::Off`]. Every line carries a number the
/// ledger recorded; estimates say so. Which fact, and which telling of it,
/// varies from call to call.
pub fn summary(measured: &Measured, estimates: &[Estimate], style: SummaryStyle) -> Option<String> {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize);
    summary_choosing(measured, estimates, style, |len| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % len.max(1)
    })
}

fn summary_choosing(
    measured: &Measured,
    estimates: &[Estimate],
    style: SummaryStyle,
    mut pick: impl FnMut(usize) -> usize,
) -> Option<String> {
    if style == SummaryStyle::Off {
        return None;
    }
    let mut facts = facts(measured, estimates);
    if facts.is_empty() {
        return None;
    }
    let fact = facts.swap_remove(pick(facts.len()) % facts.len());
    Some(match style {
        SummaryStyle::Plain => fact.plain,
        _ => {
            let mut quips = fact.quips;
            quips.swap_remove(pick(quips.len()) % quips.len())
        }
    })
}

fn facts(measured: &Measured, estimates: &[Estimate]) -> Vec<Candidate> {
    let mut facts = Vec::new();
    let waits: Vec<&Estimate> = estimates
        .iter()
        .filter(|e| matches!(e.kind, Kind::CiWait | Kind::ProcessWait) && e.refused.is_none())
        .collect();
    let n: u64 = waits.iter().map(|e| e.runs).sum();
    let tokens: u64 = waits.iter().filter_map(|e| e.input_tokens).sum();
    let polls = waits
        .iter()
        .filter_map(|e| e.requests_avoided)
        .sum::<f64>()
        .round() as u64;
    if n > 0 && tokens >= MIN_WAIT_TOKENS {
        let tok = tokens_text(tokens as f64);
        let waits = if n == 1 {
            "1 wait".to_string()
        } else {
            format!("{n} waits")
        };
        facts.push(Candidate {
            quips: vec![
                format!("jouet[savings]: {waits} instead of about {polls} polls. the context window was not re-read {polls} times (~{tok} tokens, estimated)."),
                format!("jouet[savings]: {waits}, zero 'is it done yet?'. about {tok} tokens left unread (estimated)."),
                format!("jouet[savings]: {polls} polls not made. CI did not notice; the context window did (~{tok} tokens, estimated)."),
            ],
            plain: format!("jouet[savings]: {waits} replaced about {polls} polls, an estimated {tok} input tokens"),
        });
    }
    let steered: u64 = measured.rules.iter().map(|r| r.then_jouet).sum();
    if steered > 0 {
        let cmds = if steered == 1 {
            "1 command".to_string()
        } else {
            format!("{steered} commands")
        };
        facts.push(Candidate {
            quips: vec![
                format!("jouet[savings]: rules sent {cmds} the jouet way. no one argued."),
                format!("jouet[savings]: {cmds} redirected to jouet. they went quietly."),
            ],
            plain: format!("jouet[savings]: rules steered {cmds} to jouet"),
        });
    }
    let excerpts = measured.failure_excerpts;
    if excerpts > 0 {
        let logs = if excerpts == 1 {
            "1 failing CI log".to_string()
        } else {
            format!("{excerpts} failing CI logs")
        };
        facts.push(Candidate {
            quips: vec![
                format!("jouet[savings]: {logs} arrived as the failing step. the rest stayed on disk, unread."),
                format!("jouet[savings]: {logs} trimmed to the part that failed. the passing parts were not consulted."),
            ],
            plain: format!("jouet[savings]: {logs} returned as the failing step's excerpt"),
        });
    }
    if measured.verify_blocks > 0 {
        let n = measured.verify_blocks;
        let times = if n == 1 {
            "once".to_string()
        } else {
            format!("{n} times")
        };
        facts.push(Candidate {
            quips: vec![
                format!("jouet[savings]: stopped a victory lap over a failing check {times}."),
                format!("jouet[savings]: 'done' was declared over a failing check {times}. jouet disagreed."),
            ],
            plain: format!("jouet[savings]: blocked finishing after a failed check {times}"),
        });
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::RuleCounts;

    fn wait(tokens: u64) -> Estimate {
        Estimate {
            harness: "claude".into(),
            kind: Kind::CiWait,
            runs: 3,
            input_tokens: Some(tokens),
            requests_avoided: Some(24.0),
            basis: String::new(),
            refused: None,
        }
    }

    fn lines(measured: &Measured, estimates: &[Estimate], style: SummaryStyle) -> Vec<String> {
        let mut out = Vec::new();
        for fact in 0..4 {
            for variant in 0..3 {
                let mut picks = [fact, variant].into_iter();
                if let Some(line) =
                    summary_choosing(measured, estimates, style, |_| picks.next().unwrap_or(0))
                {
                    out.push(line);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn nothing_worth_saying_says_nothing() {
        let m = Measured::default();
        assert!(lines(&m, &[wait(10)], SummaryStyle::Quips).is_empty());
        assert!(lines(&m, &[wait(5_000_000)], SummaryStyle::Off).is_empty());
    }

    #[test]
    fn every_line_carries_a_recorded_number_and_estimates_say_so() {
        let m = Measured {
            rules: vec![RuleCounts {
                rule: "sleep-poll".into(),
                denies: 2,
                then_jouet: 2,
                ..RuleCounts::default()
            }],
            failure_excerpts: 1,
            verify_blocks: 1,
            ..Measured::default()
        };
        let quips = lines(&m, &[wait(2_400_000)], SummaryStyle::Quips);
        assert_eq!(quips.len(), 9, "{quips:#?}");
        for line in &quips {
            assert!(line.starts_with("jouet[savings]: "), "{line}");
            assert!(
                line.chars().any(|c| c.is_ascii_digit()) || line.contains("once"),
                "{line}"
            );
            if line.contains("tokens") {
                assert!(line.contains("estimated"), "{line}");
            }
        }
        let plain = lines(&m, &[wait(2_400_000)], SummaryStyle::Plain);
        assert_eq!(
            plain,
            [
                "jouet[savings]: 1 failing CI log returned as the failing step's excerpt",
                "jouet[savings]: 3 waits replaced about 24 polls, an estimated 2.4M input tokens",
                "jouet[savings]: blocked finishing after a failed check once",
                "jouet[savings]: rules steered 2 commands to jouet",
            ]
        );
    }
}
