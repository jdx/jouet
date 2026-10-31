//! `jouet wind` and `jouet unwind`: drafting rules from the hook ledger, and
//! proposing the ones to retire.

use super::Output;
use crate::cli::{
    UnwindArgs, WindArgs, WindCmd, WindDigestArgs, WindMemoriesArgs, WindProposeArgs,
};
use crate::settings::Settings;
use crate::wind::{self, Paths, Status, stats, unwind as retire};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;

fn paths(out: &Output) -> Result<Paths, Exit> {
    Paths::locate().ok_or_else(|| {
        out.error(
            Exit::Failed,
            "there is no home directory to keep the ledger and rules in",
        )
    })
}

pub(crate) fn run(args: &WindArgs, out: &Output) -> Exit {
    let paths = match paths(out) {
        Ok(paths) => paths,
        Err(exit) => return exit,
    };
    match &args.cmd {
        None => wind_now(&paths, out),
        Some(WindCmd::Show(note)) => show(&paths, &note.id, out),
        Some(WindCmd::Dismiss(note)) => {
            if wind::set_status(&paths, &note.id, Status::Dismissed) {
                out.result(&serde_json::json!({ "dismissed": note.id }), || {
                    format!("dismissed {}\n", note.id)
                });
                Exit::Ok
            } else {
                out.error(Exit::NotFound, &format!("no winding note `{}`", note.id))
            }
        }
        Some(WindCmd::Adopt(note)) => adopt(&paths, &note.id, out),
        Some(WindCmd::Digest(args)) => sessions(args, false, out),
        Some(WindCmd::Review(args)) => sessions(args, true, out),
        Some(WindCmd::Propose(args)) => propose(&paths, args, out),
        Some(WindCmd::Memories(args)) => memories(&paths, args, out),
    }
}

#[derive(Serialize)]
struct WindReport<'a> {
    #[serde(flatten)]
    run: &'a wind::RunReport,
    /// Notes not yet adopted, dismissed or lapsed; `jouet wind show ID`
    /// prints one in full.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pending: Vec<Pending<'a>>,
    /// What reviews proposed, not yet adopted, dismissed or lapsed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    proposals: Vec<&'a wind::review::Proposal>,
    /// Sessions the stop hook found worth a review.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reviews: Vec<&'a wind::review::Queued>,
    /// Sequences a mise task may replace, and tasks drafted for them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tasks: Vec<&'a wind::tasks::Proposal>,
    /// Memories that look stale.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    memories: Vec<&'a wind::memories::MemoryNote>,
}

#[derive(Serialize)]
struct Pending<'a> {
    id: &'a str,
    rule: &'a str,
    why: &'a str,
}

fn wind_now(paths: &Paths, out: &Output) -> Exit {
    let settings = Settings::load();
    if !settings.wind.enabled {
        return out.error(
            Exit::Precondition,
            "winding is off; turn it on with `[wind] enabled = true` in ~/.config/jouet/config.toml, or unset JOUET_WIND",
        );
    }
    let report = wind::run(paths, &settings.wind, crate::wind::now(), true);
    let state = wind::State::load(&paths.state);
    let pending: Vec<Pending> = state
        .notes
        .iter()
        .filter(|n| n.status == Status::Pending)
        .map(|n| Pending {
            id: &n.id,
            rule: &n.rule,
            why: &n.why,
        })
        .collect();
    let doc = WindReport {
        run: &report,
        pending,
        proposals: state
            .reviews
            .proposals
            .iter()
            .filter(|p| p.status == Status::Pending)
            .collect(),
        reviews: state
            .reviews
            .queue
            .iter()
            .filter(|q| q.status == Status::Pending)
            .collect(),
        tasks: state
            .tasks
            .proposals
            .iter()
            .filter(|p| p.status == Status::Pending)
            .collect(),
        memories: state
            .memories
            .notes
            .iter()
            .filter(|n| n.status == Status::Pending)
            .collect(),
    };
    out.result(&doc, || {
        let mut text = format!("read {} ledger entries", report.read);
        if !report.drafted.is_empty() {
            let _ = write!(text, "; drafted {}", report.drafted.join(", "));
        }
        if !report.promoted.is_empty() {
            let _ = write!(text, "; promoted {} to deny", report.promoted.join(", "));
        }
        if !report.noticed.is_empty() {
            let _ = write!(
                text,
                "; noticed {} for a mise task",
                report.noticed.join(", ")
            );
        }
        text.push('\n');
        for note in &doc.pending {
            let _ = writeln!(text, "{}  {}  {}", note.id, note.rule, note.why);
        }
        for p in &doc.proposals {
            let _ = writeln!(text, "{}  {}  {}", p.id, p.kind.name(), p.title);
        }
        for p in &doc.tasks {
            match &p.task {
                Some(t) => {
                    let _ = writeln!(text, "{}  task  {}: {}", p.id, t.name, t.run);
                }
                None => {
                    let _ = writeln!(
                        text,
                        "{}  task  `{}`: `jouet task propose` in that repository drafts it",
                        p.id, p.shape
                    );
                }
            }
        }
        for n in &doc.memories {
            let _ = writeln!(text, "{}  memory  `{}` {}", n.id, n.memory, n.found);
        }
        for q in &doc.reviews {
            let _ = writeln!(
                text,
                "{}  review  session {}: `jouet wind review {}`",
                q.id, q.session, q.session
            );
        }
        if !settings.hooks.ledger {
            text.push_str("note: the ledger is off ([hooks] ledger = false), so there is nothing new to read\n");
        }
        text
    });
    Exit::Ok
}

fn show(paths: &Paths, id: &str, out: &Output) -> Exit {
    let state = wind::State::load(&paths.state);
    if let Some(p) = state.tasks.proposal(id) {
        out.result(p, || match &p.task {
            Some(t) => format!(
                "{} ({}): mise task `{}`, ran {} times in {} sessions\nrun: {}\nsuggest: {}: {}\napply: jouet task apply {} --file local|shared, after asking the user which\n",
                p.id,
                p.status.name(),
                t.name,
                t.evidence.runs,
                t.evidence.sessions,
                t.run,
                t.suggest.file.config(),
                t.suggest.reasons.join("; "),
                p.id
            ),
            None => format!(
                "{} ({}): `{}` ran {} times in {} sessions in one repository; `jouet task propose` there drafts a mise task for it\n",
                p.id,
                p.status.name(),
                p.shape,
                p.seen_runs,
                p.seen_sessions
            ),
        });
        return Exit::Ok;
    }
    if let Some(p) = state.reviews.proposal(id) {
        out.result(p, || {
            let mut text = format!(
                "{} ({}, {}, scope {}): {}\n{}\n",
                p.id,
                p.status.name(),
                p.kind.name(),
                p.scope,
                p.title,
                p.why
            );
            if !p.session.is_empty() {
                let _ = writeln!(
                    text,
                    "from a review of session {}: {}",
                    p.session,
                    p.evidence.join(", ")
                );
            }
            let _ = write!(
                text,
                "applied: jouet wind adopt {}\n\n{}\n",
                p.id,
                p.draft.trim_end()
            );
            text
        });
        return Exit::Ok;
    }
    if let Some(n) = state.memories.note(id) {
        out.result(n, || {
            format!(
                "{} ({}): memory `{}` {}\npath: {}\nwhen it is dealt with: jouet wind adopt {}\n",
                n.id,
                n.status.name(),
                n.memory,
                n.found,
                n.path,
                n.id
            )
        });
        return Exit::Ok;
    }
    if let Some(q) = state.reviews.queued(id) {
        out.result(q, || {
            format!(
                "{} ({}): session {} ({}) is worth a review: {} corrections, {} failed retries, {} polling\nreview: jouet wind review {}\n",
                q.id,
                q.status.name(),
                q.session,
                q.harness,
                q.corrections,
                q.failed_retries,
                jouet_audit::digest::duration(q.polling_ms),
                q.session
            )
        });
        return Exit::Ok;
    }
    let Some(note) = state.note(id) else {
        return out.error(Exit::NotFound, &format!("no winding note `{id}`"));
    };
    out.result(note, || {
        let script = match &note.script {
            Some(script) => format!("\n-- {}.luau, its script:\n{script}", note.rule),
            None => String::new(),
        };
        format!(
            "{} ({}): {}\nseen {} times in {} sessions\nadopt: jouet rule new {} --from-note {}\n\n{}{script}",
            note.id,
            note.status.name(),
            note.why,
            note.count,
            note.sessions,
            note.rule,
            note.id,
            note.code
        )
    });
    Exit::Ok
}

/// Marks a review's proposal or a memory note as dealt with, so it is not
/// shown again.
fn adopt(paths: &Paths, id: &str, out: &Output) -> Exit {
    let state = wind::State::load(&paths.state);
    if state.note(id).is_some() {
        return out.error(
            Exit::Usage,
            &format!(
                "`{id}` is a drafted rule; adopt it with `jouet rule new NAME --from-note {id}`"
            ),
        );
    }
    if state.tasks.proposal(id).is_some() {
        return out.error(
            Exit::Usage,
            &format!(
                "`{id}` is a mise task; add it with `jouet task apply {id} --file local|shared`"
            ),
        );
    }
    if state.reviews.proposal(id).is_none() && state.memories.note(id).is_none() {
        return out.error(Exit::NotFound, &format!("no proposal `{id}`"));
    }
    if !wind::set_status(paths, id, Status::Adopted) {
        return out.error(Exit::Failed, "could not write the winding state");
    }
    out.result(&serde_json::json!({ "adopted": id }), || {
        format!("adopted {id}\n")
    });
    Exit::Ok
}

#[derive(Serialize)]
struct DigestReport<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    redaction: String,
    digests: &'a [jouet_audit::digest::Digest],
    #[serde(skip_serializing_if = "is_empty_across")]
    across: &'a jouet_audit::digest::Across,
    /// Sessions written since `--since` but left out, older than the ones
    /// digested.
    #[serde(skip_serializing_if = "is_zero")]
    left_out: usize,
    /// For a review, the memories its sessions' agent reads.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    memories: Vec<wind::memories::Shown>,
}

fn is_empty_across(a: &&jouet_audit::digest::Across) -> bool {
    a.files.is_empty() && a.commands.is_empty()
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Tokens per session's digest, unless --budget says otherwise.
const BUDGET: usize = 4000;

/// `jouet wind digest` and, with `prompt`, `jouet wind review`.
fn sessions(args: &WindDigestArgs, prompt: bool, out: &Output) -> Exit {
    use wind::session::{self, Redactor, Select};
    let since = match args.since.as_deref() {
        None => None,
        Some(s) => match super::audit::parse_since(s, jouet_output::time::now()) {
            Ok(secs) => Some(secs.saturating_mul(1000)),
            Err(e) => return out.error(Exit::Usage, &format!("--since: {e}")),
        },
    };
    let select = match (args.session.as_deref(), args.last, since) {
        (Some(name), false, None) => Select::Named(name),
        (None, true, None) => Select::Last,
        (None, false, Some(ms)) => Select::Since(ms),
        (None, false, None) => {
            return out.error(
                Exit::Usage,
                "name a session (an ID, a prefix of one or a transcript's path), or pass --last or --since",
            );
        }
        _ => return out.error(Exit::Usage, "give a session, --last or --since, only one"),
    };
    let project = match args.project.as_deref().map(std::path::absolute) {
        None => None,
        Some(Ok(path)) => Some(super::audit::lexical(&path)),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--project: {e}")),
    };
    let readers = match session::Readers::from_env(args.harness.as_deref()) {
        Ok(r) => r,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let (found, left_out) = match session::find(&readers, &select, project.as_deref()) {
        Ok(found) => found,
        Err(e) => return out.error(Exit::NotFound, &e),
    };
    let redactor = Redactor::new(Settings::load().wind.gitleaks.as_deref());
    if let Some(problem) = &redactor.problem {
        eprintln!("jouet: not using gitleaks' rules: {problem}");
    }
    let digests: Vec<_> = found
        .iter()
        .filter_map(|f| session::digest(f, project.as_deref(), &redactor))
        .collect();
    if digests.is_empty() {
        return out.error(Exit::NotFound, "no session there to digest");
    }
    for d in &digests {
        crate::share::record_digest(d);
    }
    let budget = args.budget.unwrap_or(BUDGET).max(200);
    let across = jouet_audit::digest::across(&digests);
    let memories = if prompt {
        review_memories(&digests, &redactor)
    } else {
        Vec::new()
    };
    let report = DigestReport {
        prompt: prompt.then(|| wind::review::prompt(Settings::load().share.contribute)),
        redaction: redactor.describe(),
        digests: &digests,
        across: &across,
        left_out,
        memories,
    };
    out.result(&report, || {
        let mut text = String::new();
        if prompt {
            let ids: Vec<&str> = digests.iter().map(|d| d.session.as_str()).collect();
            let prompt = report.prompt.as_deref().unwrap_or_default();
            text.push_str(&prompt.replace("SESSION_ID", &ids.join(", ")));
            text.push_str("\n---\n\n");
        }
        let _ = writeln!(text, "Redacted with {}; local only.\n", report.redaction);
        if digests.len() == 1 {
            text.push_str(&jouet_audit::digest::render(&digests[0], budget));
        } else {
            text.push_str(&jouet_audit::digest::render_all(
                &digests,
                &across,
                budget * digests.len(),
            ));
        }
        if left_out > 0 {
            let _ = writeln!(
                text,
                "… {left_out} older sessions since then left out; narrow --since or name one"
            );
        }
        text.push_str(&wind::memories::render(&report.memories));
        text
    });
    Exit::Ok
}

#[derive(Serialize)]
struct MemoriesReport {
    /// Memories read, and the Claude Code projects they belong to.
    memories: usize,
    projects: usize,
    /// PRs and issues asked about this time; the rest were known.
    asked: usize,
    /// Why GitHub was not asked, when it could not be.
    #[serde(skip_serializing_if = "Option::is_none")]
    github: Option<String>,
    findings: Vec<wind::memories::Finding>,
    /// Findings recorded as notes this run, with --record.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recorded: Vec<String>,
}

/// The built-in rules' names, which a memory may say to get past.
fn builtin_rules(paths: &Paths) -> Vec<String> {
    wind::user_engine(&paths.rules)
        .rules()
        .iter()
        .filter(|r| r.scope == jouet_rules::Scope::Builtin)
        .map(|r| r.name.clone())
        .collect()
}

/// `jouet wind memories`: what in the agents' memories looks stale.
fn memories(paths: &Paths, args: &WindMemoriesArgs, out: &Output) -> Exit {
    use wind::memories::{self as m, Kind};
    let settings = Settings::load();
    if !settings.wind.enabled || !settings.wind.memories {
        return out.error(
            Exit::Precondition,
            "memory upkeep is off; turn it on with `[wind] memories = true` (and `enabled`), or unset JOUET_WIND_MEMORIES and JOUET_WIND",
        );
    }
    let all = m::discover(&m::Roots::from_env());
    let checked = m::extract(&all, &builtin_rules(paths));
    let now = wind::now();
    let known = wind::State::load(&paths.state).memories.lookups;
    let wanted = m::to_look_up(&checked, &known, now);
    let mut github = None;
    let fresh = if wanted.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        let mut client = jouet_github::Client::from_env();
        m::look_up(&mut client, &wanted, &known, now).unwrap_or_else(|e| {
            github = Some(e);
            std::collections::BTreeMap::new()
        })
    };
    let mut lookups = known;
    lookups.extend(fresh.clone());
    let found = m::findings(&checked, &lookups, &|words| crate::spec::unknown_in(words));
    // A memory that works around jouet is a report for its maintainers,
    // when the user contributes.
    let mut ideas = std::collections::BTreeMap::new();
    if args.record
        && settings.share.contribute
        && let Some(gaps) = crate::share::gaps_path()
    {
        let redactor = wind::session::Redactor::new(settings.wind.gitleaks.as_deref());
        for f in found.iter().filter(|f| f.kind == Kind::Jouet) {
            let sentence = checked
                .iter()
                .filter(|c| c.memory.path.to_string_lossy() == f.path)
                .flat_map(|c| &c.claims.overrides)
                .find(|o| o.0 == f.about)
                .map(|o| redactor.redact(&o.1))
                .unwrap_or_default();
            let title = format!(
                "A saved memory works around the built-in rule `{}`",
                f.about
            );
            let body = format!(
                "An agent saved a memory to get past `{}`, so the rule may misfire: \"{sentence}\"",
                f.about
            );
            if let Ok(key) = crate::share::record_review(&gaps, &title, &body, now) {
                ideas.insert(f.path.clone(), key);
            }
        }
    }
    let referenced: std::collections::BTreeSet<&str> = checked
        .iter()
        .flat_map(|c| c.claims.references.iter().map(|r| r.key.as_str()))
        .collect();
    let recorded = wind::update(&paths.state, |state| {
        state.memories.lookups.extend(fresh);
        state
            .memories
            .lookups
            .retain(|key, _| referenced.contains(key.as_str()));
        if args.record {
            m::record(state, &found, &ideas, now)
        } else {
            Vec::new()
        }
    })
    .unwrap_or_default();
    let report = MemoriesReport {
        memories: all.len(),
        projects: all
            .iter()
            .filter_map(|m| m.project.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        asked: wanted.len(),
        github,
        findings: found,
        recorded,
    };
    out.result(&report, || {
        let mut text = format!(
            "read {} memories ({} projects); asked GitHub about {} references\n",
            report.memories, report.projects, report.asked
        );
        if let Some(e) = &report.github {
            let _ = writeln!(text, "GitHub was not asked: {e}");
        }
        if report.findings.is_empty() {
            text.push_str("nothing looks stale\n");
        }
        for f in &report.findings {
            let _ = writeln!(
                text,
                "{}  {}\n  {}",
                f.memory,
                m::tilde(std::path::Path::new(&f.path)),
                f.found
            );
        }
        if !report.recorded.is_empty() {
            let _ = writeln!(
                text,
                "recorded {}; the next session starts in their projects show them",
                report.recorded.join(", ")
            );
        }
        text
    });
    Exit::Ok
}

/// The memories a review of `digests` shows: their projects' and the
/// global ones, whole when a session read one or a memory note names it.
fn review_memories(
    digests: &[jouet_audit::digest::Digest],
    redactor: &wind::session::Redactor,
) -> Vec<wind::memories::Shown> {
    use wind::memories as m;
    let settings = Settings::load();
    if !settings.wind.memories {
        return Vec::new();
    }
    let home = jouet_audit::home();
    let expand = |p: &str| match (p.strip_prefix("~/"), &home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(p),
    };
    let projects: Vec<std::path::PathBuf> = digests
        .iter()
        .filter_map(|d| d.project.as_deref().map(expand))
        .collect();
    let all = m::discover(&m::Roots::from_env());
    let ours = m::for_projects(&all, &projects);
    let read: std::collections::BTreeSet<String> = digests
        .iter()
        .flat_map(jouet_audit::digest::Digest::files_read)
        .map(str::to_string)
        .collect();
    let noted: std::collections::BTreeSet<String> = Paths::locate()
        .map(|p| wind::State::load(&p.state).memories.notes)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.status == Status::Pending)
        .map(|n| n.path)
        .collect();
    m::for_review(&ours, &read, &noted, &|text| redactor.redact(text))
}

/// `jouet wind propose`: records a review's reply.
fn propose(paths: &Paths, args: &WindProposeArgs, out: &Output) -> Exit {
    use std::io::Read as _;
    let settings = Settings::load();
    if !settings.wind.enabled {
        return out.error(
            Exit::Precondition,
            "winding is off; turn it on with `[wind] enabled = true`, or unset JOUET_WIND",
        );
    }
    let text = match args.file.as_deref() {
        Some(file) => match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => return out.error(Exit::NotFound, &format!("reading {file}: {e}")),
        },
        None => {
            let mut text = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut text) {
                return out.error(Exit::Failed, &format!("reading stdin: {e}"));
            }
            text
        }
    };
    // A reviewer may wrap the JSON in a fenced block.
    let json = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let reply: wind::review::Reply = match serde_json::from_str(json) {
        Ok(reply) => reply,
        Err(e) => {
            return out.error(
                Exit::Usage,
                &format!("the reply is not the JSON `jouet wind review` asks for: {e}"),
            );
        }
    };
    let targets: Vec<String> = wind::memories::discover(&wind::memories::Roots::from_env())
        .iter()
        .map(wind::memories::Memory::display)
        .collect();
    let recorded = match wind::review::record(paths, reply, crate::wind::now(), &targets) {
        Ok(recorded) => recorded,
        Err(e) => return out.error(Exit::Failed, &format!("writing the winding state: {e}")),
    };
    out.result(&recorded, || {
        let mut text = String::new();
        for id in &recorded.recorded {
            let _ = writeln!(text, "recorded {id}; the next session starts show it");
        }
        for key in &recorded.ideas {
            let _ = writeln!(
                text,
                "recorded idea {key} for jouet's maintainers; `jouet share --idea {key}` drafts it, and posts only with --yes"
            );
        }
        for s in &recorded.skipped {
            let _ = writeln!(text, "skipped \"{}\": {}", s.title, s.reason);
        }
        if recorded.recorded.is_empty() && recorded.skipped.is_empty() && recorded.ideas.is_empty() {
            text.push_str(if recorded.nothing {
                "nothing to record: the review found nothing worth changing\n"
            } else {
                "nothing to record: the reply has no proposals\n"
            });
        }
        text
    });
    Exit::Ok
}

#[derive(Serialize)]
struct UnwindReport {
    proposals: Vec<retire::Proposal>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    retired: Vec<wind::rulefile::Change>,
    /// Mise tasks jouet added that no session runs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tasks: Vec<UnusedTask>,
    /// The ones `--apply` removed, as `mise tasks rm --json` reported them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed_tasks: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

#[derive(Serialize)]
struct UnusedTask {
    task: String,
    path: String,
    reason: String,
    #[serde(skip)]
    root: String,
}

/// The tasks jouet added that no session has run, and that still run what
/// jouet wrote: one someone changed is theirs now.
fn unused_tasks(paths: &Paths, now: u64) -> Vec<UnusedTask> {
    let state = wind::State::load(&paths.state);
    let mut out = Vec::new();
    for a in state.tasks.unused(now) {
        let root = std::path::Path::new(&a.root);
        let Ok(existing) = crate::mise::tasks(root) else {
            continue;
        };
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let unchanged = existing
            .iter()
            .any(|t| t.name == a.name && t.run.iter().map(|r| squash(r)).eq([squash(&a.run)]));
        if !unchanged {
            continue;
        }
        let days = now.saturating_sub(a.at) / (24 * 60 * 60);
        out.push(UnusedTask {
            task: a.name.clone(),
            path: a.path.clone(),
            reason: format!(
                "jouet task apply added it {days} days ago ({}), and no agent session has run it since",
                a.proposal
            ),
            root: a.root.clone(),
        });
    }
    out
}

/// Removes each task with `mise tasks rm`, and forgets it.
fn remove_tasks(paths: &Paths, tasks: &[UnusedTask]) -> (Vec<serde_json::Value>, Vec<String>) {
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for t in tasks {
        let args: Vec<String> = ["tasks", "rm", "--json", "--yes", "--path", &t.path, &t.task]
            .iter()
            .map(|a| (*a).to_string())
            .collect();
        match crate::mise::json(std::path::Path::new(&t.root), &args) {
            Ok(value) => {
                let _ = wind::update(&paths.state, |state| {
                    state
                        .tasks
                        .applied
                        .retain(|a| !(a.root == t.root && a.name == t.task));
                });
                // One report per task removed.
                match value {
                    serde_json::Value::Array(reports) => removed.extend(reports),
                    other => removed.push(other),
                }
            }
            Err(problem) => errors.push(format!("removing task {}: {}", t.task, problem.message())),
        }
    }
    (removed, errors)
}

pub(crate) fn unwind(args: &UnwindArgs, out: &Output) -> Exit {
    let paths = match paths(out) {
        Ok(paths) => paths,
        Err(exit) => return exit,
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut loaded = crate::rules::load(&cwd);
    loaded.engine.load_scripts();
    let stats = stats::compute(&wind::reader::read_all(&paths.ledger));
    let proposals = retire::proposals(&loaded.engine, &stats);
    let now = crate::wind::now();
    let tasks = unused_tasks(&paths, now);
    let (retired, mut errors) = if args.apply {
        retire::apply(&paths, &proposals, "user", now)
    } else {
        (Vec::new(), Vec::new())
    };
    let removed_tasks = if args.apply {
        let (removed, task_errors) = remove_tasks(&paths, &tasks);
        errors.extend(task_errors);
        removed
    } else {
        Vec::new()
    };
    let report = UnwindReport {
        proposals,
        retired,
        tasks,
        removed_tasks,
        errors,
    };
    out.result(&report, || {
        if report.proposals.is_empty() && report.tasks.is_empty() {
            return "nothing to unwind: no rule of yours is noisy, nearly always overridden or broken, and no task jouet added goes unused\n".into();
        }
        let mut text = String::new();
        for t in &report.tasks {
            let field = |r: &serde_json::Value, key: &str| {
                r.get(key).and_then(serde_json::Value::as_str).map(str::to_string)
            };
            let done = report.removed_tasks.iter().any(|r| {
                field(r, "task").as_ref() == Some(&t.task) && field(r, "path").as_ref() == Some(&t.path)
            });
            let mark = if done { "removed " } else { "propose " };
            let _ = writeln!(text, "{mark} task {} ({})  {}", t.task, t.path, t.reason);
        }
        if !args.apply && !report.tasks.is_empty() {
            text.push_str("run `jouet unwind --apply` to remove the tasks listed (`mise tasks rm`), after asking the user\n");
        }
        for p in &report.proposals {
            let done = report.retired.iter().any(|c| c.rule == p.rule);
            let mark = match (done, p.can_apply) {
                (true, _) => "retired ",
                (false, true) => "propose ",
                (false, false) => "repo    ",
            };
            let _ = writeln!(text, "{mark} {}  {}", p.rule, p.reason);
        }
        if !args.apply && report.proposals.iter().any(|p| p.can_apply) {
            text.push_str("run `jouet unwind --apply` to retire the rules of yours listed\n");
        }
        if report.proposals.iter().any(|p| !p.can_apply) {
            text.push_str("a repository's rules change through a PR to its .jouet/rules\n");
        }
        for e in &report.errors {
            let _ = writeln!(text, "error: {e}");
        }
        text
    });
    if report.errors.is_empty() {
        Exit::Ok
    } else {
        Exit::Failed
    }
}
