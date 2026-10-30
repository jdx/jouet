//! `jouet wind` and `jouet unwind`: drafting rules from the hook ledger, and
//! proposing the ones to retire.

use super::Output;
use crate::settings::Settings;
use crate::wind::{self, Paths, Status, stats, unwind as retire};
use crate::{UnwindArgs, WindArgs, WindCmd, WindDigestArgs, WindProposeArgs};
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
    };
    out.result(&doc, || {
        let mut text = format!("read {} ledger entries", report.read);
        if !report.drafted.is_empty() {
            let _ = write!(text, "; drafted {}", report.drafted.join(", "));
        }
        if !report.promoted.is_empty() {
            let _ = write!(text, "; promoted {} to deny", report.promoted.join(", "));
        }
        text.push('\n');
        for note in &doc.pending {
            let _ = writeln!(text, "{}  {}  {}", note.id, note.rule, note.why);
        }
        for p in &doc.proposals {
            let _ = writeln!(text, "{}  {}  {}", p.id, p.kind.name(), p.title);
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
        format!(
            "{} ({}): {}\nseen {} times in {} sessions\nadopt: jouet rule new {} --from-note {}\n\n{}",
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

/// Marks a review's proposal as applied, so it is not shown again.
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
    if state.reviews.proposal(id).is_none() {
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
    prompt: Option<&'static str>,
    redaction: String,
    digests: &'a [jouet_audit::digest::Digest],
    #[serde(skip_serializing_if = "is_empty_across")]
    across: &'a jouet_audit::digest::Across,
    /// Sessions written since `--since` but left out, older than the ones
    /// digested.
    #[serde(skip_serializing_if = "is_zero")]
    left_out: usize,
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
    let budget = args.budget.unwrap_or(BUDGET).max(200);
    let across = jouet_audit::digest::across(&digests);
    let report = DigestReport {
        prompt: prompt.then_some(wind::review::PROMPT),
        redaction: redactor.describe(),
        digests: &digests,
        across: &across,
        left_out,
    };
    out.result(&report, || {
        let mut text = String::new();
        if prompt {
            let ids: Vec<&str> = digests.iter().map(|d| d.session.as_str()).collect();
            text.push_str(&wind::review::PROMPT.replace("SESSION_ID", &ids.join(", ")));
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
        text
    });
    Exit::Ok
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
    let recorded = match wind::review::record(paths, reply, crate::wind::now()) {
        Ok(recorded) => recorded,
        Err(e) => return out.error(Exit::Failed, &format!("writing the winding state: {e}")),
    };
    out.result(&recorded, || {
        let mut text = String::new();
        for id in &recorded.recorded {
            let _ = writeln!(text, "recorded {id}; the next session starts show it");
        }
        for s in &recorded.skipped {
            let _ = writeln!(text, "skipped \"{}\": {}", s.title, s.reason);
        }
        if recorded.recorded.is_empty() && recorded.skipped.is_empty() {
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

pub(crate) fn unwind(args: &UnwindArgs, out: &Output) -> Exit {
    let paths = match paths(out) {
        Ok(paths) => paths,
        Err(exit) => return exit,
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let loaded = match crate::rules::load(&cwd) {
        Ok(loaded) => loaded,
        Err(e) => return out.error(Exit::Failed, &format!("starting the rule engine: {e}")),
    };
    let stats = stats::compute(&wind::reader::read_all(&paths.ledger));
    let proposals = retire::proposals(&loaded.engine, &stats);
    let (retired, errors) = if args.apply {
        retire::apply(&paths, &proposals, "user", crate::wind::now())
    } else {
        (Vec::new(), Vec::new())
    };
    let report = UnwindReport {
        proposals,
        retired,
        errors,
    };
    out.result(&report, || {
        if report.proposals.is_empty() {
            return "nothing to unwind: no rule of yours is noisy, nearly always overridden or broken\n".into();
        }
        let mut text = String::new();
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
