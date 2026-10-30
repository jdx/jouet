//! Sharing proven rules, missing primitives, and what sessions show about
//! jouet itself with jouet's maintainers.
//!
//! Contributing is opt-in and always goes through a person: `jouet share`
//! builds a proposal, prints the exact text and a diff of what anonymizing
//! changed, and posts nothing. Only `--yes` posts it with the user's own
//! GitHub login: a bug ([`signals`] such as a crash) as an Issue on
//! `jdx/jouet`, anything else as a Discussion in its "Rules & ideas"
//! category. Before posting it looks for an existing proposal, refuses
//! anything that still looks like a secret, and posts at most [`MAX_POSTS`]
//! a week.

pub(crate) mod anonymize;
pub(crate) mod signals;

use crate::wind::{self, Paths, replay, rulefile, stats};
use jouet_rules::{Engine, Gap, Mode, Scope, State};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) const OWNER: &str = "jdx";
pub(crate) const REPO: &str = "jouet";
pub(crate) const CATEGORY: &str = "Rules & ideas";

/// What a rule needs before it can be shared.
pub(crate) const MIN_DENY_DAYS: u64 = 7;
pub(crate) const MIN_FIRINGS: u64 = 10;
pub(crate) const MAX_OVERRIDE_RATE: f64 = 0.1;
/// Proposals posted per week, at most.
pub(crate) const MAX_POSTS: usize = 3;
const WEEK: u64 = 7 * 86_400;

/// The primitives rules reached for, and wishes agents wrote down.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GapEntry {
    /// A missing field of the `jouet` module, such as `pr_status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primitive: Option<String>,
    /// The rule that reached for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// What an agent could not express, in its own words; for a change a
    /// session review proposed, its why and draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A change to jouet a session review proposed: its title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub count: u64,
    pub first: u64,
    pub last: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
    /// What a session showed about jouet itself, instead of a primitive or
    /// a note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<signals::Signal>,
    /// Sessions a start-of-session line suggested sharing it to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

impl GapEntry {
    /// What `jouet share --idea KEY` names it by.
    pub(crate) fn key(&self) -> String {
        if let Some(signal) = &self.signal {
            return signal.key();
        }
        match (&self.primitive, self.title.as_ref().or(self.note.as_ref())) {
            (Some(p), _) => p.clone(),
            (None, Some(note)) => {
                let words: Vec<&str> = note.split_whitespace().take(5).collect();
                let slug: String = words
                    .join("-")
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                format!("note-{}", slug.to_lowercase())
            }
            (None, None) => String::new(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Gaps {
    #[serde(default)]
    pub gaps: Vec<GapEntry>,
}

/// `$XDG_STATE_HOME/jouet/gaps.json`.
pub(crate) fn gaps_path() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("gaps.json"))
}

/// `$XDG_STATE_HOME/jouet/shared.jsonl`: what was posted, for the weekly
/// limit.
pub(crate) fn posted_path() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("shared.jsonl"))
}

fn load<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Changes the gap file under a lock, as hooks of several sessions may
/// record gaps at once.
pub(crate) fn update_gaps<T>(
    path: &Path,
    change: impl FnOnce(&mut Gaps) -> T,
) -> std::io::Result<T> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut gaps: Gaps = load(path);
    let out = change(&mut gaps);
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, &gaps)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(out)
}

pub(crate) fn read_gaps(path: &Path) -> Gaps {
    load(path)
}

fn add_gap(gaps: &mut Gaps, entry: GapEntry, now: u64) {
    if let Some(existing) = gaps.gaps.iter_mut().find(|g| {
        g.signal.is_none()
            && g.primitive == entry.primitive
            && g.rule == entry.rule
            && g.note == entry.note
    }) {
        existing.count += 1;
        existing.last = now;
        return;
    }
    gaps.gaps.push(GapEntry {
        count: 1,
        first: now,
        last: now,
        ..entry
    });
}

/// Counts the ledger's signals about jouet afresh into the gap file.
pub(crate) fn refresh_signals(gaps_path: &Path, paths: &Paths) -> std::io::Result<()> {
    let entries = wind::reader::read_all(&paths.ledger);
    let engine = wind::user_engine(&paths.rules);
    let fresh = signals::from_ledger(&entries, Some(&engine));
    update_gaps(gaps_path, |gaps| signals::replace_ledger(gaps, fresh))
}

/// Records what a digest shows about jouet itself. Best effort; a session
/// digested again adds nothing.
pub(crate) fn record_digest(digest: &jouet_audit::digest::Digest) {
    if digest.jouet.is_empty() {
        return;
    }
    let Some(path) = gaps_path() else { return };
    let found: Vec<_> = digest
        .jouet
        .iter()
        .map(|j| (j.found.clone(), j.command.clone(), j.span.from))
        .collect();
    let _ = update_gaps(&path, |gaps| {
        signals::add_digest(gaps, &digest.session, &found);
    });
}

/// The line a starting session shows about an idea worth sharing, when
/// the user opted in with `[share] contribute`: the most recorded one not
/// yet shared, shown to at most three sessions. Bugs come first.
pub(crate) fn line_for_session(gaps_path: &Path, session: &str) -> Option<String> {
    update_gaps(gaps_path, |gaps| {
        let mut ready: Vec<&mut GapEntry> = gaps
            .gaps
            .iter_mut()
            .filter(|g| !g.shared && g.shown.len() < wind::MAX_SHOWINGS)
            .filter(|g| !g.shown.iter().any(|s| s == session))
            .filter(|g| g.signal.as_ref().is_some_and(signals::Signal::ready))
            .collect();
        ready.sort_by_key(|g| {
            let s = g.signal.as_ref().expect("filtered to signals");
            (
                s.destination() != signals::Destination::Issue,
                std::cmp::Reverse(s.count),
            )
        });
        let entry = ready.into_iter().next()?;
        entry.shown.push(session.to_string());
        let signal = entry.signal.as_ref().expect("filtered to signals");
        let place = match signal.destination() {
            signals::Destination::Issue => "an Issue",
            signals::Destination::Discussion => "a Discussion",
        };
        Some(format!(
            "jouet noticed something for its maintainers: {}. `jouet share --idea {}` drafts {place} on jdx/jouet; show the draft to the user, and post it with `--yes` only if they agree.",
            signal.summary(),
            entry.key()
        ))
    })
    .ok()
    .flatten()
}

/// Records the primitives an engine's rules reached for. Best effort.
pub(crate) fn record_gaps(engine: &Engine) {
    let gaps = engine.take_gaps();
    if gaps.is_empty() {
        return;
    }
    if let Some(path) = gaps_path() {
        let _ = record(&path, &gaps, wind::now());
    }
}

pub(crate) fn record(path: &Path, found: &[Gap], now: u64) -> std::io::Result<()> {
    update_gaps(path, |gaps| {
        for gap in found {
            let entry = GapEntry {
                primitive: Some(gap.primitive.clone()),
                rule: (!gap.rule.is_empty()).then(|| gap.rule.clone()),
                ..GapEntry::default()
            };
            add_gap(gaps, entry, now);
        }
    })
}

/// Records what an agent could not express, in its words.
pub(crate) fn record_note(
    path: &Path,
    note: &str,
    rule: Option<&str>,
    now: u64,
) -> std::io::Result<()> {
    update_gaps(path, |gaps| {
        let entry = GapEntry {
            note: Some(note.trim().to_string()),
            rule: rule.map(str::to_string),
            ..GapEntry::default()
        };
        add_gap(gaps, entry, now);
    })
}

/// Records a change to jouet a session review proposed.
pub(crate) fn record_review(
    path: &Path,
    title: &str,
    body: &str,
    now: u64,
) -> std::io::Result<String> {
    let entry = GapEntry {
        title: Some(title.trim().to_string()),
        note: Some(body.trim().to_string()),
        ..GapEntry::default()
    };
    let key = entry.key();
    update_gaps(path, |gaps| {
        if !gaps.gaps.iter().any(|g| g.title == entry.title) {
            add_gap(gaps, entry, now);
        }
    })?;
    Ok(key)
}

/// What a rule has done, gathered for a proposal.
#[derive(Debug, Serialize)]
pub(crate) struct Evidence {
    pub deny_days: u64,
    pub fired: u64,
    pub overridden: u64,
    pub sessions: u64,
    /// Its `fires` and `quiet` examples, all passing.
    pub fires: usize,
    pub quiet: usize,
    pub replay_fired: u64,
    pub replay_calls: u64,
    #[serde(skip)]
    pub fires_on: Vec<replay::Hit>,
    #[serde(skip)]
    pub near_misses: Vec<replay::Hit>,
}

/// What sharing a rule publishes of it: its `[[rule]]` table (or a Luau
/// rule file), and the script it names, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleText {
    /// The rule's TOML, or a whole Luau rule file.
    pub code: String,
    pub format: jouet_rules::Format,
    /// The script's path, as the rule names it, and its code.
    pub script: Option<(String, String)>,
}

/// A rule of yours as sharing publishes it: only its own table from a file
/// that holds several, and its script.
fn shared_code(rule: &jouet_rules::Rule) -> Result<RuleText, String> {
    let jouet_rules::Origin::User(path) = &rule.origin else {
        return Err(format!("`{}` has no file of yours", rule.name));
    };
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))
    };
    let file = read(path)?;
    let code = match rule.format {
        jouet_rules::Format::Luau => file,
        jouet_rules::Format::Toml => match rulefile::block(&file, &rule.name) {
            Some(range) => file[range].to_string(),
            None => file,
        },
    };
    let script = match &rule.script {
        Some(script) => {
            let dir = path.parent().unwrap_or(Path::new("."));
            Some((script.clone(), read(&dir.join(script))?))
        }
        None => None,
    };
    Ok(RuleText {
        code,
        format: rule.format,
        script,
    })
}

/// Checks a rule of yours against what sharing needs, returning its
/// evidence, its `why` and what is published of it, or every reason it is
/// not ready.
pub(crate) fn check_rule(
    paths: &Paths,
    name: &str,
    now: u64,
) -> Result<(Evidence, String, RuleText), Vec<String>> {
    let mut engine = wind::user_engine(&paths.rules);
    engine.load_scripts();
    let Some(rule) = engine.rule(name) else {
        return Err(vec![format!("no rule named `{name}`")]);
    };
    if rule.scope != Scope::User {
        return Err(vec![format!(
            "`{name}` is a {} rule; only your own rules are shared",
            rule.scope.name()
        )]);
    }
    if let State::Broken(e) = &rule.state {
        return Err(vec![format!("`{name}` does not load: {e}")]);
    }
    let mut problems = Vec::new();
    if let Err(problem) = engine.proven(rule) {
        problems.push(format!(
            "{problem}; it needs passing examples of both kinds, `fires` and `quiet`: check them with `jouet rule test {name}`"
        ));
    }
    let entries = wind::reader::read_all(&paths.ledger);
    let replayed = replay::replay(&engine, rule, &entries);
    if replayed.fired_total == 0 {
        problems.push("`jouet rule test --replay` finds no call in the ledger it fires on".into());
    }
    let promoted = rulefile::read_log(&paths.log)
        .into_iter()
        .filter(|c| c.rule == name && c.change == "promoted")
        .map(|c| c.ts)
        .max();
    let deny_days = match (rule.mode, promoted) {
        (Mode::Deny, Some(ts)) => now.saturating_sub(ts) / 86_400,
        _ => 0,
    };
    if rule.mode != Mode::Deny {
        problems.push(format!("it is in warn mode; `jouet rule promote {name}` it and let it deny for {MIN_DENY_DAYS} days"));
    } else if promoted.is_none() {
        problems.push(format!("the rule log has no promotion for it; deny time counts from `jouet rule promote {name}`"));
    } else if deny_days < MIN_DENY_DAYS {
        problems.push(format!(
            "it has denied for {deny_days} days; sharing needs {MIN_DENY_DAYS}"
        ));
    }
    let s = stats::compute(&entries).get(name);
    if s.fired < MIN_FIRINGS {
        problems.push(format!(
            "it has fired {} times; sharing needs {MIN_FIRINGS}",
            s.fired
        ));
    }
    if s.fired > 0 && s.override_rate() > MAX_OVERRIDE_RATE {
        problems.push(format!(
            "{} of its {} firings were overridden; sharing needs at most {:.0}%",
            s.overridden,
            s.fired,
            MAX_OVERRIDE_RATE * 100.0
        ));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let code = shared_code(rule).map_err(|e| vec![e])?;
    let examples = engine.examples(rule).unwrap_or_default();
    let fires = examples.iter().filter(|e| e.fires).count();
    Ok((
        Evidence {
            deny_days,
            fired: s.fired,
            overridden: s.overridden,
            sessions: s.sessions,
            fires,
            quiet: examples.len() - fires,
            replay_fired: replayed.fired_total,
            replay_calls: replayed.calls,
            fires_on: replayed.fired,
            near_misses: replayed.near_misses,
        },
        rule.why.clone(),
        code,
    ))
}

/// A proposal: what would be posted, and what it was before anonymizing.
#[derive(Debug, Serialize)]
pub(crate) struct Proposal {
    pub kind: &'static str,
    /// An Issue for a bug, else a Discussion in "Rules & ideas".
    pub destination: signals::Destination,
    pub title: String,
    /// The text that would be posted.
    pub body: String,
    /// What anonymizing changed, as a unified diff; empty when nothing did.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub diff: String,
}

/// The AI disclosure line every post ends with: the config's
/// `reply_footer` when there is one, else one naming the detected agent.
pub(crate) fn footer() -> String {
    if let Some(footer) = crate::config::Config::load()
        .ok()
        .and_then(|c| c.reply_footer())
    {
        return footer;
    }
    let tool =
        mise_agent_env::detect().map_or_else(|| "jouet".to_string(), |d| d.agent.to_string());
    let version = std::env::var("AI_AGENT")
        .ok()
        .filter(|v| !v.is_empty() && v != "0")
        .unwrap_or_else(|| "unavailable".into());
    format!(
        "*AI-assisted — Tool: {tool} (via jouet share); model: unavailable; version: {version}.*"
    )
}

fn finish(
    kind: &'static str,
    destination: signals::Destination,
    title: String,
    original: String,
    words: &[String],
    footer: &str,
) -> Proposal {
    let title = anonymize::anonymize(&title, words);
    let body = format!("{}\n{footer}\n", anonymize::anonymize(&original, words));
    let original = format!("{original}\n{footer}\n");
    let diff = if body == original {
        String::new()
    } else {
        similar::TextDiff::from_lines(&original, &body)
            .unified_diff()
            .context_radius(1)
            .header("local", "published")
            .to_string()
    };
    Proposal {
        kind,
        destination,
        title,
        body,
        diff,
    }
}

fn shapes(out: &mut String, hits: &[replay::Hit]) {
    for hit in hits {
        let _ = writeln!(out, "- `{}` ({}x)", hit.command, hit.count);
    }
}

pub(crate) fn rule_proposal(
    name: &str,
    why: &str,
    text: &RuleText,
    e: &Evidence,
    words: &[String],
    footer: &str,
) -> Proposal {
    let lang = match text.format {
        jouet_rules::Format::Toml => "toml",
        jouet_rules::Format::Luau => "lua",
    };
    let mut body = format!(
        "A rule of mine that has proven itself, proposed for jouet's built-in rules or an official rule pack.\n\n**Why:** {why}\n\n### The rule\n\n```{lang}\n{}\n```\n\n",
        text.code.trim()
    );
    if let Some((path, script)) = &text.script {
        let _ = writeln!(
            body,
            "### Its script, `{path}`\n\n```lua\n{}\n```\n",
            script.trim_end()
        );
    }
    let _ = writeln!(body, "### How it has done\n");
    let _ = writeln!(body, "- In deny mode for {} days.", e.deny_days);
    let _ = writeln!(
        body,
        "- Fired {} times in {} sessions; overridden {} times ({:.0}%).",
        e.fired,
        e.sessions,
        e.overridden,
        if e.fired == 0 {
            0.0
        } else {
            e.overridden as f64 * 100.0 / e.fired as f64
        }
    );
    let _ = writeln!(
        body,
        "- `jouet rule test`: {} `fires` and {} `quiet` examples pass. `--replay` fires on {} of {} calls in the ledger.",
        e.fires, e.quiet, e.replay_fired, e.replay_calls
    );
    body.push_str("\n### Calls it fires on (shapes, values replaced)\n\n");
    shapes(&mut body, &e.fires_on);
    if !e.near_misses.is_empty() {
        body.push_str("\n### Near misses it allows\n\n");
        shapes(&mut body, &e.near_misses);
    }
    finish(
        "rule",
        signals::Destination::Discussion,
        format!("Rule: {name}"),
        body,
        words,
        footer,
    )
}

pub(crate) fn idea_proposal(
    key: &str,
    entries: &[&GapEntry],
    why: &[String],
    words: &[String],
    footer: &str,
) -> Proposal {
    let count: u64 = entries.iter().map(|g| g.count).sum();
    let rules: Vec<&str> = entries.iter().filter_map(|g| g.rule.as_deref()).collect();
    let primitive = entries.iter().find_map(|g| g.primitive.as_deref());
    let mut body = String::new();
    let title = match primitive {
        Some(p) => {
            let _ = writeln!(
                body,
                "My rules reached for `jouet.{p}`, which the rule API does not have, {count} times{}.\n",
                if rules.is_empty() {
                    String::new()
                } else {
                    format!(" (in {})", rules.join(", "))
                }
            );
            for w in why {
                let _ = writeln!(body, "- I kept trying to catch: {w}");
            }
            if !why.is_empty() {
                body.push('\n');
            }
            let _ = writeln!(
                body,
                "There is no primitive for `jouet.{p}`. With one, these rules could be written against the public API."
            );
            format!("Idea: a `jouet.{p}` primitive")
        }
        None if entries.iter().any(|g| g.title.is_some()) => {
            let entry = entries.iter().find(|g| g.title.is_some()).expect("found");
            let title = entry.title.as_deref().unwrap_or(key);
            let _ = writeln!(
                body,
                "A review of one of my agent sessions suggests this change to jouet:\n"
            );
            for line in entry.note.as_deref().unwrap_or_default().lines() {
                let _ = writeln!(body, "{line}");
            }
            format!("Idea: {title}")
        }
        None => {
            let note = entries
                .first()
                .and_then(|g| g.note.as_deref())
                .unwrap_or(key);
            let _ = writeln!(
                body,
                "A rule I wanted could not be expressed with jouet's rule API ({count} times):\n"
            );
            for line in note.lines() {
                let _ = writeln!(body, "> {line}");
            }
            let first = note.lines().next().unwrap_or(note);
            let short: String = first.chars().take(60).collect();
            format!("Idea: {short}")
        }
    };
    finish(
        "idea",
        signals::Destination::Discussion,
        title,
        body,
        words,
        footer,
    )
}

/// A proposal from what sessions showed about jouet itself: a bug report
/// with the steps that show it, or an idea for a Discussion.
pub(crate) fn signal_proposal(
    signal: &signals::Signal,
    words: &[String],
    footer: &str,
) -> Proposal {
    use signals::Kind;
    let subject = &signal.subject;
    let sessions = signal.sessions.len();
    let mut body = String::new();
    let title = match signal.kind() {
        Kind::Crash => format!("Bug: `{subject}` panicked"),
        Kind::ExitCode => format!("Bug: `{subject}` exited outside the exit-code table"),
        Kind::BadJson => format!("Bug: `{subject} --json` printed something that is not JSON"),
        Kind::SlowHook => format!("Bug: the {subject} hook runs over its time budget"),
        Kind::Overridden if signal.destination() == signals::Destination::Issue => {
            format!("Bug: built-in rule `{subject}` is overridden most of the time")
        }
        Kind::Overridden => format!("Idea: built-in rule `{subject}` may fire where it should not"),
        Kind::Usage => format!("Idea: agents keep hitting usage errors with `{subject}`"),
        Kind::Abandoned => format!("Idea: agents drop `{subject}` for raw commands"),
        Kind::Routine => format!(
            "Idea: a built-in for a script agents keep writing ({})",
            &signal.key()["routine-".len()..]
        ),
    };
    let _ = writeln!(body, "{}.\n", capitalize(&signal.summary()));
    let what = match signal.kind() {
        Kind::Crash | Kind::ExitCode | Kind::BadJson => {
            "jouet's own sessions on my machine show it; the commands below are shapes, with every value replaced, so they show the flags but not the arguments."
        }
        Kind::Usage => {
            "Agents ran it, got exit 2 (a usage error), and often retried with other flags: the CLI or `jouet agent-help` may be missing what they needed."
        }
        Kind::Abandoned => {
            "Right after it answered, the agent ran the raw command it replaces: its output may be missing something the agent needed."
        }
        Kind::Overridden => {
            "Agents ran a command of the same shape again within ten minutes of the rule denying or warning about it. Each override is a call the rule may have stopped wrongly."
        }
        Kind::SlowHook => {
            "jouet records a hook that runs over its budget: every tool call waits for it."
        }
        Kind::Routine => {
            "The same script, with values replaced, ran in each of these sessions: it may want to be a jouet command or a built-in routine."
        }
    };
    let _ = writeln!(body, "{what}\n");
    if !signal.details.is_empty() {
        body.push_str("### What was seen\n\n");
        for (detail, n) in &signal.details {
            let _ = writeln!(body, "- {detail} ({n}x)");
        }
        body.push('\n');
    }
    let examples: Vec<&String> = if signal.kind() == Kind::Routine {
        vec![&signal.subject]
    } else {
        signal.examples.iter().collect()
    };
    if !examples.is_empty() {
        body.push_str(match signal.destination() {
            signals::Destination::Issue => "### To reproduce\n\n",
            signals::Destination::Discussion => "### The commands (shapes)\n\n",
        });
        for e in examples {
            let _ = writeln!(body, "- `{e}`");
        }
        body.push('\n');
    }
    if let (Kind::SlowHook, [budget, slowest, ..]) = (signal.kind(), &signal.numbers[..]) {
        let _ = writeln!(body, "Budget {budget} ms; slowest {slowest} ms.\n");
    }
    let _ = writeln!(
        body,
        "Seen in {sessions} sessions. jouet {} on {}.",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    );
    let kind = match signal.destination() {
        signals::Destination::Issue => "bug",
        signals::Destination::Discussion => "idea",
    };
    finish(kind, signal.destination(), title, body, words, footer)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Every candidate idea, by key, most recorded first.
pub(crate) fn ideas(gaps: &Gaps) -> Vec<(String, Vec<&GapEntry>)> {
    let mut by_key: Vec<(String, Vec<&GapEntry>)> = Vec::new();
    for gap in gaps
        .gaps
        .iter()
        .filter(|g| !g.shared)
        .filter(|g| g.signal.as_ref().is_none_or(signals::Signal::ready))
    {
        let key = gap.key();
        match by_key.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(gap),
            None => by_key.push((key, vec![gap])),
        }
    }
    by_key.sort_by_key(|(k, list)| {
        (
            std::cmp::Reverse(list.iter().map(|g| g.count).sum::<u64>()),
            k.clone(),
        )
    });
    by_key
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Posted {
    pub ts: u64,
    pub kind: String,
    pub title: String,
    pub url: String,
}

/// How many proposals were posted in the week before `now`.
pub(crate) fn posted_this_week(path: &Path, now: u64) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Posted>(l).ok())
        .filter(|p| now.saturating_sub(p.ts) < WEEK)
        .count()
}

pub(crate) fn note_posted(path: &Path, posted: &Posted) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_vec(posted)?;
    line.push(b'\n');
    std::io::Write::write_all(
        &mut std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?,
        &line,
    )
}

/// Why posting stopped before anything was written.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The category does not exist yet.
    NoCategory,
    Duplicate(String),
    Remote(String),
}

/// Looks up the repository (and, for a Discussion, its category), refuses
/// a duplicate, and creates the Issue or Discussion. Returns the poster's
/// login and the URL.
pub(crate) fn post(
    client: &mut jouet_github::Client,
    proposal: &Proposal,
) -> Result<(String, String), Refusal> {
    let issue = proposal.destination == signals::Destination::Issue;
    let data = client
        .graphql(
            "query($owner:String!,$name:String!,$q:String!,$type:SearchType!){viewer{login} repository(owner:$owner,name:$name){id discussionCategories(first:50){nodes{id name}}} search(query:$q,type:$type,first:10){nodes{... on Discussion{title url} ... on Issue{title url}}}}",
            &json!({
                "owner": OWNER,
                "name": REPO,
                "q": format!(
                    "repo:{OWNER}/{REPO} {}in:title \"{}\"",
                    if issue { "is:issue " } else { "" },
                    proposal.title.replace('"', "")
                ),
                "type": if issue { "ISSUE" } else { "DISCUSSION" },
            }),
        )
        .map_err(Refusal::Remote)?;
    let login = data["viewer"]["login"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let repo = &data["repository"];
    let Some(repo_id) = repo["id"].as_str() else {
        return Err(Refusal::Remote(format!("{OWNER}/{REPO} was not found")));
    };
    let duplicate = data["search"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| {
            d["title"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(&proposal.title))
        });
    if let Some(d) = duplicate {
        return Err(Refusal::Duplicate(
            d["url"].as_str().unwrap_or_default().to_string(),
        ));
    }
    if issue {
        let created = client
            .mutate(
                "mutation($repo:ID!,$title:String!,$body:String!){createIssue(input:{repositoryId:$repo,title:$title,body:$body}){issue{url}}}",
                &json!({ "repo": repo_id, "title": proposal.title, "body": proposal.body }),
            )
            .map_err(Refusal::Remote)?;
        let url = created["createIssue"]["issue"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        return Ok((login, url));
    }
    let category = repo["discussionCategories"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| {
            c["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(CATEGORY))
        });
    let Some(category_id) = category.and_then(|c| c["id"].as_str()) else {
        return Err(Refusal::NoCategory);
    };
    let created = client
        .mutate(
            "mutation($repo:ID!,$cat:ID!,$title:String!,$body:String!){createDiscussion(input:{repositoryId:$repo,categoryId:$cat,title:$title,body:$body}){discussion{url}}}",
            &json!({ "repo": repo_id, "cat": category_id, "title": proposal.title, "body": proposal.body }),
        )
        .map_err(Refusal::Remote)?;
    let url = created["createDiscussion"]["discussion"]["url"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok((login, url))
}

#[cfg(test)]
mod tests;
