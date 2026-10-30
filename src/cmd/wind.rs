//! `jouet wind` and `jouet unwind`: drafting rules from the hook ledger, and
//! proposing the ones to retire.

use super::Output;
use crate::exit::Exit;
use crate::settings::Settings;
use crate::wind::{self, Paths, Status, stats, unwind as retire};
use crate::{UnwindArgs, WindArgs, WindCmd};
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

pub fn run(args: &WindArgs, out: &Output) -> Exit {
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
        if !settings.hooks.ledger {
            text.push_str("note: the ledger is off ([hooks] ledger = false), so there is nothing new to read\n");
        }
        text
    });
    Exit::Ok
}

fn show(paths: &Paths, id: &str, out: &Output) -> Exit {
    let state = wind::State::load(&paths.state);
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

#[derive(Serialize)]
struct UnwindReport {
    proposals: Vec<retire::Proposal>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    retired: Vec<wind::rulefile::Change>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

pub fn unwind(args: &UnwindArgs, out: &Output) -> Exit {
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
