//! A digest as text for a reviewer, within a token budget.
//!
//! The sections most likely to hold an improvement come first:
//! corrections, failures, waits, then where the tokens went, re-read files
//! and retyped commands, and the timeline last. When the text is over
//! budget, lists and the timeline are cut in steps, each cut ending with a
//! marker naming what was left out, until it fits or nothing is left to
//! cut. The budget counts four characters as a token.

use super::{Across, Digest, Group, Phase, Span};
use std::fmt::Write as _;

/// How much of each section is shown.
#[derive(Clone, Copy)]
struct Limits {
    items: usize,
    groups: usize,
    details: usize,
    /// Phases shown in full; the rest are listed only when they hold a
    /// correction, and otherwise counted.
    phases: usize,
}

const STEPS: [Limits; 6] = [
    Limits {
        items: 12,
        groups: 6,
        details: 3,
        phases: usize::MAX,
    },
    Limits {
        items: 8,
        groups: 4,
        details: 2,
        phases: usize::MAX,
    },
    Limits {
        items: 6,
        groups: 3,
        details: 1,
        phases: 40,
    },
    Limits {
        items: 5,
        groups: 2,
        details: 1,
        phases: 16,
    },
    Limits {
        items: 4,
        groups: 1,
        details: 0,
        phases: 8,
    },
    Limits {
        items: 3,
        groups: 0,
        details: 0,
        phases: 4,
    },
];

/// The digest as text, within about `budget` tokens when it can be.
pub fn render(d: &Digest, budget: usize) -> String {
    let mut text = String::new();
    for limits in STEPS {
        text = render_with(d, limits);
        if text.len() / 4 <= budget {
            break;
        }
    }
    text
}

/// Several digests, with what they share first, each within an equal part
/// of `budget`.
pub fn render_all(digests: &[Digest], shared: &Across, budget: usize) -> String {
    let mut out = String::new();
    if !shared.files.is_empty() || !shared.commands.is_empty() {
        let _ = writeln!(out, "# Across {} sessions\n", digests.len());
        let items = 10;
        if !shared.files.is_empty() {
            out.push_str("Files read in several sessions:\n");
            list(&mut out, &shared.files, items, |s| {
                format!("- {} — {} sessions, {} reads", s.what, s.sessions, s.times)
            });
        }
        if !shared.commands.is_empty() {
            out.push_str("Commands typed out in several sessions:\n");
            list(&mut out, &shared.commands, items, |s| {
                format!("- `{}` — {} sessions, {} runs", s.what, s.sessions, s.times)
            });
        }
        out.push('\n');
    }
    let each = budget.saturating_sub(out.len() / 4) / digests.len().max(1);
    for d in digests {
        out.push_str(&render(d, each));
        out.push('\n');
    }
    out
}

fn span(s: Span) -> String {
    if s.from == s.to {
        format!("L{}", s.from)
    } else {
        format!("L{}-{}", s.from, s.to)
    }
}

/// Up to `max` items, then a marker for the rest.
fn list<T>(out: &mut String, items: &[T], max: usize, line: impl Fn(&T) -> String) {
    for item in items.iter().take(max) {
        out.push_str(&line(item));
        out.push('\n');
    }
    if items.len() > max {
        let _ = writeln!(out, "- … {} more", items.len() - max);
    }
}

fn render_with(d: &Digest, l: Limits) -> String {
    let mut out = String::new();
    let t = &d.totals;
    let _ = write!(out, "# Session {} ({})", d.session, d.harness);
    if let Some(project) = &d.project {
        let _ = write!(out, " in {project}");
    }
    out.push('\n');
    let _ = write!(out, "{}", d.started_ms.map(date).unwrap_or_default());
    let _ = writeln!(
        out,
        ", {} wall, {} prompts, {} requests, {} calls ({} failed), {} input tokens, {} output, {} in tools",
        duration(t.wall_ms),
        t.prompts,
        t.requests,
        t.calls,
        t.failed,
        tokens(t.input_tokens),
        tokens(t.output_tokens),
        duration(t.tool_ms),
    );
    let _ = writeln!(out, "Transcript: {} (L = its line numbers)\n", d.transcript);

    if !d.corrections.is_empty() {
        let _ = writeln!(out, "## User corrections ({})", d.corrections.len());
        list(&mut out, &d.corrections, l.items, |c| {
            let mut line = format!("- {} {}", span(c.span), c.kind.name());
            if c.after_interrupt {
                line.push_str(" (after an interrupt)");
            }
            if !c.text.is_empty() {
                let _ = write!(line, ": \"{}\"", c.text);
            }
            if let Some(after) = &c.after {
                let _ = write!(line, " — after `{after}`");
            }
            line
        });
        out.push('\n');
    }
    if !d.failures.is_empty() {
        let _ = writeln!(out, "## Failures ({})", d.failures.len());
        list(&mut out, &d.failures, l.items, |f| {
            let mut line = format!(
                "- {} `{}` failed {} of {}",
                span(f.span),
                f.what,
                f.failed,
                f.attempts
            );
            line.push_str(if f.recovered {
                ", then worked"
            } else if f.attempts > f.failed {
                ", failing last"
            } else {
                ", never worked"
            });
            if let Some(error) = &f.error {
                let _ = write!(line, ": {error}");
            }
            line
        });
        out.push('\n');
    }
    if !d.jouet.is_empty() {
        let _ = writeln!(out, "## jouet itself ({})", d.jouet.len());
        list(&mut out, &d.jouet, l.items, |j| {
            format!(
                "- {} `{}` {}: {}",
                span(j.span),
                j.found.subject,
                j.found.kind.name(),
                j.found.detail
            )
        });
        out.push('\n');
    }
    if !d.waits.is_empty() {
        let total: u64 = d.waits.iter().map(|w| w.duration_ms).sum();
        let _ = writeln!(
            out,
            "## Long calls ({}, {} in all)",
            d.waits.len(),
            duration(total)
        );
        list(&mut out, &d.waits, l.items, |w| {
            format!(
                "- {} `{}` {}",
                span(w.span),
                w.what,
                duration(w.duration_ms)
            )
        });
        out.push('\n');
    }
    if !d.sinks.is_empty() {
        out.push_str("## Where tokens and time went\n");
        list(&mut out, &d.sinks, l.items.min(6), |s| {
            let mut line = format!(
                "- {} {:.0}%: {} tokens, {} calls, {}",
                s.category,
                s.share * 100.0,
                tokens(s.input_tokens),
                s.calls,
                duration(s.duration_ms)
            );
            if let Some(shape) = &s.top_shape {
                let _ = write!(line, " (most: `{shape}`)");
            }
            line
        });
        out.push('\n');
    }
    if !d.rereads.is_empty() {
        let _ = writeln!(out, "## Files read repeatedly ({})", d.rereads.len());
        list(&mut out, &d.rereads, l.items, |r| {
            format!(
                "- {} {} reads, {} with no edit since the last — {}",
                r.file,
                r.reads,
                r.unchanged,
                span(r.span)
            )
        });
        out.push('\n');
    }
    if !d.retyped.is_empty() {
        let _ = writeln!(out, "## Commands typed out again ({})", d.retyped.len());
        list(&mut out, &d.retyped, l.items, |r| {
            format!("- {}× `{}` — {}", r.runs, r.command, span(r.span))
        });
        out.push('\n');
    }
    if !d.patterns.is_empty() {
        out.push_str("## Patterns jouet audit knows\n");
        list(&mut out, &d.patterns, l.items, |p| {
            format!("- {} ({} calls): {}", p.pattern, p.calls, p.advice)
        });
        out.push('\n');
    }
    timeline(&mut out, d, l);
    out
}

fn timeline(out: &mut String, d: &Digest, l: Limits) {
    let _ = writeln!(out, "## Timeline ({} phases)", d.phases.len());
    let corrected = |p: &Phase| {
        d.corrections
            .iter()
            .any(|c| c.span.to >= p.span.from && c.span.from <= p.span.to)
    };
    // The first and last phases in full, and any with a correction.
    let n = d.phases.len();
    let head = l.phases.div_ceil(2);
    let tail = l.phases / 2;
    let mut skipped = (0u64, 0u64);
    for (i, p) in d.phases.iter().enumerate() {
        let full = n <= l.phases || i < head || i >= n.saturating_sub(tail);
        if !full && !corrected(p) {
            skipped.0 += 1;
            skipped.1 += p.calls.iter().map(|g| g.count).sum::<u64>();
            continue;
        }
        if skipped.0 > 0 {
            let _ = writeln!(
                out,
                "- … {} phases left out ({} calls)",
                skipped.0, skipped.1
            );
            skipped = (0, 0);
        }
        phase(out, i + 1, p, l);
    }
    if skipped.0 > 0 {
        let _ = writeln!(
            out,
            "- … {} phases left out ({} calls)",
            skipped.0, skipped.1
        );
    }
}

fn phase(out: &mut String, n: usize, p: &Phase, l: Limits) {
    let calls: u64 = p.calls.iter().map(|g| g.count).sum();
    let _ = write!(out, "{n}. {}", span(p.span));
    if let Some(offset) = p.offset_ms {
        let _ = write!(out, " +{}", duration(u64::try_from(offset).unwrap_or(0)));
    }
    if let Some(d) = p.duration_ms {
        let _ = write!(out, " for {}", duration(d));
    }
    let _ = write!(
        out,
        ", {} requests, {} tokens, {calls} calls",
        p.requests,
        tokens(p.input_tokens)
    );
    match &p.prompt {
        Some(prompt) => {
            let _ = writeln!(out, ": \"{prompt}\"");
        }
        None => out.push_str(" (before the first prompt)\n"),
    }
    for g in p.calls.iter().take(l.groups) {
        group(out, g, l.details);
    }
    if p.calls.len() > l.groups && l.groups > 0 {
        let rest: u64 = p.calls[l.groups..].iter().map(|g| g.count).sum();
        let _ = writeln!(
            out,
            "   … {} more kinds of call ({rest} calls)",
            p.calls.len() - l.groups
        );
    }
}

fn group(out: &mut String, g: &Group, details: usize) {
    let _ = write!(out, "   `{}` ×{}", g.what, g.count);
    if g.failed > 0 {
        let _ = write!(out, " ({} failed)", g.failed);
    }
    let shown: Vec<&str> = g.details.iter().take(details).map(String::as_str).collect();
    if !shown.is_empty() {
        let _ = write!(out, ": {}", shown.join(", "));
    }
    let more = g.more + (g.details.len() - shown.len()) as u64;
    if more > 0 && !shown.is_empty() {
        let _ = write!(out, " (+{more})");
    }
    out.push('\n');
}

/// `850ms`, `12s`, `4m10s`, `2h05m`.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0 => format!("{ms}ms"),
        1..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// `950`, `48k`, `1.2M`.
fn tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// `2026-10-30 10:00 UTC` for milliseconds since the Unix epoch.
fn date(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_sizes_read_at_a_glance() {
        assert_eq!(duration(850), "850ms");
        assert_eq!(duration(12_400), "12s");
        assert_eq!(duration(250_000), "4m10s");
        assert_eq!(duration(7_500_000), "2h05m");
        assert_eq!(tokens(48_200), "48k");
        assert_eq!(tokens(1_250_000), "1.2M");
        assert_eq!(date(1_793_354_400_000), "2026-10-30 10:00 UTC");
    }
}
