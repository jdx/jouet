//! Self-winding: jouet noticing costly patterns in its own hook ledger and
//! drafting rules against them, the way an agent keeps memories.
//!
//! `jouet wind` reads the ledger from where it last stopped, feeds the new
//! entries to the [`detect::Detector`], and turns each pattern that crosses
//! its threshold into a **note**: a one-line explanation and a drafted rule
//! in warn mode. The session-start hook shows at most [`MAX_PER_START`]
//! pending notes; an agent adopts one with `jouet rule new NAME --from-note
//! ID`, which writes the rule under the user's rules. Nothing is enabled
//! without that step, and every change goes to `jouet rule log`.

pub(crate) mod detect;
pub(crate) mod draft;
pub(crate) mod reader;
pub(crate) mod replay;
pub(crate) mod review;
pub(crate) mod rulefile;
pub(crate) mod session;
pub(crate) mod stats;
pub(crate) mod tasks;
pub(crate) mod unwind;

use crate::ledger;
use crate::settings::WindSettings;
use detect::{Detector, Pattern};
use jouet_rules::{Call, Engine, Event, Mode, Scope};
use reader::Cursor;
use rulefile::Change;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Notes shown when a session starts, at most. Not a setting: more would
/// crowd out the session's own work.
pub(crate) const MAX_PER_START: usize = 2;
/// Sessions a note is shown to before it lapses.
pub(crate) const MAX_SHOWINGS: usize = 3;
/// New notes drafted per day, at most.
pub(crate) const MAX_NEW_PER_DAY: usize = 2;
/// Runs closer together than this read nothing: the stop hook fires at the
/// end of every turn.
const MIN_INTERVAL: u64 = 60;

/// Seconds since the Unix epoch, as the ledger and the rule log keep time.
pub(crate) fn now() -> u64 {
    u64::try_from(jouet_output::time::now()).unwrap_or(0)
}

/// Where winding reads and writes; one place, so tests can point it at a
/// scratch directory.
#[derive(Debug, Clone)]
pub(crate) struct Paths {
    pub ledger: PathBuf,
    pub state: PathBuf,
    pub rules: PathBuf,
    pub log: PathBuf,
}

impl Paths {
    /// The real locations, when there is a home directory.
    pub(crate) fn locate() -> Option<Self> {
        let state = jouet_output::paths::state_dir()?;
        Some(Self {
            ledger: ledger::dir()?,
            state: state.join("wind.json"),
            rules: crate::rules::user_dir()?,
            log: rulefile::log_path()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Pending,
    Adopted,
    Dismissed,
    /// Shown [`MAX_SHOWINGS`] times without being adopted.
    Lapsed,
}

impl Status {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Adopted => "adopted",
            Self::Dismissed => "dismissed",
            Self::Lapsed => "lapsed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Note {
    pub id: String,
    pub created: u64,
    pub pattern: Pattern,
    /// The command's program, subcommands and flags, such as `gh pr view
    /// --json`.
    pub shape: String,
    /// The drafted rule's name and header line.
    pub rule: String,
    pub why: String,
    pub message: String,
    /// The drafted rule file, in TOML (in Luau, from a jouet before rules
    /// were TOML).
    pub code: String,
    /// Its script, when a matcher cannot express it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    pub count: u64,
    pub sessions: u64,
    pub status: Status,
    /// Sessions the note was shown to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub cursor: Cursor,
    #[serde(default)]
    pub last_run: u64,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub notes: Vec<Note>,
    /// Patterns already drafted, so each is drafted once.
    #[serde(default)]
    pub noted: BTreeSet<String>,
    #[serde(default)]
    pub detector: Detector,
    /// Session reviews: the sessions queued for one, and what reviews
    /// proposed.
    #[serde(default)]
    pub reviews: review::Reviews,
    /// Mise tasks: sequences noticed, tasks drafted, and the ones applied.
    #[serde(default)]
    pub tasks: tasks::Tasks,
}

impl State {
    pub(crate) fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut file, self)?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub(crate) fn note(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }
}

/// Loads, changes and saves the winding state under an exclusive lock, so
/// two sessions' hooks never each save a copy without the other's changes.
pub(crate) fn update<T>(path: &Path, change: impl FnOnce(&mut State) -> T) -> std::io::Result<T> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut state = State::load(path);
    let out = change(&mut state);
    state.save(path)?;
    Ok(out)
}

/// The built-in and user rules, which decide what counts as a check and
/// which rules exist. A repository's rules are left out: winding writes
/// only user rules.
pub(crate) fn user_engine(rules_dir: &Path) -> Engine {
    let (sources, _) =
        jouet_rules::discover(Some(rules_dir), None, &jouet_rules::TrustStore::load(None));
    Engine::new(
        sources,
        jouet_rules::Options {
            has_command: Box::new(crate::spec::has_command),
            disabled: Vec::new(),
            budget: jouet_rules::Budget::default(),
            cache_dir: None,
        },
    )
}

/// Whether the post-tool-use rules record a script as a test, lint or build.
fn is_check(engine: &Engine, script: &str) -> bool {
    let call = Call {
        event: Some(Event::PostToolUse),
        harness: "wind".into(),
        tool: "Bash".into(),
        command: Some(script.to_string()),
        path: None,
        cwd: None,
        exit_code: Some(1),
        ..Call::default()
    };
    !engine.evaluate(&call).records.is_empty()
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct RunReport {
    /// Ledger entries read this run.
    pub read: usize,
    /// Notes drafted this run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafted: Vec<String>,
    /// Rules promoted to deny this run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub promoted: Vec<String>,
    /// Sequences noticed this run that a mise task may replace.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub noticed: Vec<String>,
    /// True when the last run was too recent and this one read nothing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
}

/// One winding pass: read the new ledger entries, draft notes for patterns
/// that crossed their thresholds, and promote rules if the settings allow.
pub(crate) fn run(paths: &Paths, settings: &WindSettings, now: u64, force: bool) -> RunReport {
    let engine = user_engine(&paths.rules);
    let result = update(&paths.state, |state| {
        let mut report = RunReport::default();
        if !force && now.saturating_sub(state.last_run) < MIN_INTERVAL {
            report.skipped = true;
            return report;
        }
        let (entries, cursor) = reader::read_since(&paths.ledger, &state.cursor);
        report.read = entries.len();
        for entry in &entries {
            state
                .detector
                .observe(entry, &|script| is_check(&engine, script));
            state.tasks.observe(entry);
        }
        state.detector.finish(now);
        state.cursor = cursor;
        state.last_run = now;
        report.drafted = draft_notes(state, paths, &engine, now);
        report.noticed = state.tasks.notice(now);
        report
    });
    let mut report = result.unwrap_or_default();
    if settings.auto_promote && !report.skipped {
        report.promoted = auto_promote(paths, settings.promote_after, now);
    }
    report
}

/// Turns ready patterns into notes, within the daily limit.
fn draft_notes(state: &mut State, paths: &Paths, engine: &Engine, now: u64) -> Vec<String> {
    let today = jouet_output::time::date(now);
    let mut budget = MAX_NEW_PER_DAY.saturating_sub(
        state
            .notes
            .iter()
            .filter(|n| jouet_output::time::date(n.created) == today)
            .count(),
    );
    let ready: Vec<(String, detect::Evidence)> = state
        .detector
        .ready()
        .filter(|(key, _)| !state.noted.contains(*key))
        .map(|(k, e)| (k.clone(), e.clone()))
        .collect();
    let mut drafted = Vec::new();
    // Read only when there is something to draft, for its near misses.
    let mut ledger: Option<Vec<ledger::Entry>> = None;
    for (key, evidence) in ready {
        if budget == 0 {
            break;
        }
        let name = free_name(&draft::name_for(&evidence), state, paths, engine);
        let d = draft::draft(&evidence, &name, &today, &[]);
        let entries = ledger.get_or_insert_with(|| reader::read_all(&paths.ledger));
        let near = draft::near_misses(&d, &evidence, entries);
        let d = draft::draft(&evidence, &name, &today, &near);
        // A draft that fails its own examples (it would warn about what
        // worked, say) is never offered; the pattern is not drafted again.
        if draft::verify(&d).is_err() {
            state.noted.insert(key);
            continue;
        }
        state.next_id += 1;
        let id = format!("n{}", state.next_id);
        state.notes.push(Note {
            id: id.clone(),
            created: now,
            pattern: evidence.pattern,
            shape: evidence.signature.text(),
            rule: d.name,
            why: d.why,
            message: d.message,
            code: d.code,
            script: d.script,
            count: evidence.count,
            sessions: evidence.sessions.len() as u64,
            status: Status::Pending,
            shown: Vec::new(),
        });
        state.noted.insert(key);
        drafted.push(id);
        budget -= 1;
    }
    drafted
}

/// A rule name no file, rule or pending note already has.
fn free_name(base: &str, state: &State, paths: &Paths, engine: &Engine) -> String {
    let taken = |name: &str| {
        ["toml", "luau"]
            .iter()
            .any(|ext| paths.rules.join(format!("{name}.{ext}")).exists())
            || engine.rule(name).is_some()
            || state.notes.iter().any(|n| n.rule == name)
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_string())
}

/// The notes to show a session that is starting, marked as shown to it.
pub(crate) fn notes_for_session(paths: &Paths, session: &str) -> Vec<Note> {
    update(&paths.state, |state| {
        let mut shown = Vec::new();
        for note in state.notes.iter_mut() {
            if shown.len() == MAX_PER_START {
                break;
            }
            if note.status != Status::Pending {
                continue;
            }
            if !note.shown.iter().any(|s| s == session) {
                note.shown.push(session.to_string());
            }
            shown.push(note.clone());
            if note.shown.len() >= MAX_SHOWINGS {
                note.status = Status::Lapsed;
            }
        }
        shown
    })
    .unwrap_or_default()
}

/// A note as the session-start primer shows it: one line.
pub(crate) fn note_line(note: &Note, session: &str) -> String {
    let author = if session.is_empty() {
        String::new()
    } else {
        format!(" --author {session}")
    };
    format!(
        "jouet wind noticed: {} It drafted rule `{}` (warn mode): `jouet wind show {}` to read it, `jouet rule new {} --from-note {}{author}` to adopt it, `jouet wind dismiss {}` to drop it.",
        note.why, note.rule, note.id, note.rule, note.id, note.id
    )
}

/// Sets the status of a note, a proposal, a queued review or a task
/// proposal; false when there is no such one.
pub(crate) fn set_status(paths: &Paths, id: &str, status: Status) -> bool {
    update(&paths.state, |state| {
        let Some(note) = state.notes.iter_mut().find(|n| n.id == id) else {
            return state.reviews.set_status(id, status) || state.tasks.set_status(id, status);
        };
        note.status = status;
        true
    })
    .unwrap_or(false)
}

/// Switches a user rule to deny mode and logs it. Built-in and repository
/// rules are refused: the first are part of jouet, the second change
/// through a reviewed PR.
pub(crate) fn promote(
    paths: &Paths,
    name: &str,
    by: &str,
    reason: &str,
    now: u64,
) -> Result<Change, String> {
    let engine = user_engine(&paths.rules);
    let rule = engine
        .rule(name)
        .ok_or_else(|| format!("no rule named `{name}`"))?;
    if rule.scope != Scope::User {
        return Err(format!(
            "`{name}` is a {} rule; only your own rules (in {}) are promoted here",
            rule.scope.name(),
            paths.rules.display()
        ));
    }
    if let jouet_rules::State::Broken(e) = &rule.state {
        return Err(format!("`{name}` does not load: {e}"));
    }
    if rule.mode == Mode::Deny {
        return Err(format!("`{name}` is already in deny mode"));
    }
    // A rule denies only once it has shown, on examples of both kinds, what
    // it fires on and what it leaves alone.
    if let Err(problem) = engine.proven(rule) {
        return Err(format!(
            "`{name}` cannot deny until it passes examples of both kinds ({problem}); `jouet rule test {name}` shows them"
        ));
    }
    let jouet_rules::Origin::User(path) = &rule.origin else {
        return Err(format!("`{name}` has no file of yours"));
    };
    let code =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let code = match rule.format {
        jouet_rules::Format::Toml => rulefile::set_mode(&code, name, Mode::Deny)?,
        jouet_rules::Format::Luau => rulefile::set_luau_mode(&code, Mode::Deny),
    };
    std::fs::write(path, code).map_err(|e| format!("writing {}: {e}", path.display()))?;
    let change = Change {
        ts: now,
        rule: name.to_string(),
        change: "promoted".into(),
        from: Some("warn".into()),
        to: Some("deny".into()),
        reason: reason.to_string(),
        by: by.to_string(),
    };
    rulefile::log(&paths.log, &change).map_err(|e| format!("writing the rule log: {e}"))?;
    Ok(change)
}

/// Promotes every user rule in warn mode that has fired `after` times in at
/// least two sessions without an override.
fn auto_promote(paths: &Paths, after: u64, now: u64) -> Vec<String> {
    let engine = user_engine(&paths.rules);
    let stats = stats::compute(&reader::read_all(&paths.ledger));
    let candidates: Vec<String> = engine
        .rules()
        .iter()
        .filter(|r| r.scope == Scope::User && r.mode == Mode::Warn && r.event == Event::PreToolUse)
        .filter(|r| r.state == jouet_rules::State::Enabled)
        .filter(|r| {
            let s = stats.get(&r.name);
            s.fired >= after.max(1) && s.overridden == 0 && s.sessions >= 2
        })
        .map(|r| r.name.clone())
        .collect();
    candidates
        .into_iter()
        .filter(|name| {
            let s = stats.get(name);
            let reason = format!(
                "fired {} times in {} sessions with no overrides (auto_promote, promote_after = {after})",
                s.fired, s.sessions
            );
            promote(paths, name, "jouet wind", &reason, now).is_ok()
        })
        .collect()
}

#[cfg(test)]
mod tests;
