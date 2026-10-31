//! `jouet share [RULE] [--idea] [--yes]`: proposing a proven rule, or a
//! missing primitive, to jouet's maintainers.

use super::Output;
use crate::cli::ShareArgs;
use crate::share::{self, Posted, Proposal, Refusal, anonymize};
use crate::wind::{self, Paths, rulefile};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;

#[derive(Serialize)]
struct Report<'a> {
    #[serde(flatten)]
    proposal: &'a Proposal,
    posted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

pub(crate) fn run(args: &ShareArgs, out: &Output) -> Exit {
    let Some(paths) = Paths::locate() else {
        return out.error(
            Exit::Failed,
            "there is no home directory, so no rules or ledger",
        );
    };
    let mut words = anonymize::local_words();
    words.extend(args.redact.iter().cloned());
    let footer = share::footer();
    let now = wind::now();
    if (args.idea || args.list)
        && let Some(path) = share::gaps_path()
        && let Err(e) = share::refresh_signals(&path, &paths)
    {
        eprintln!("jouet: reading the ledger for signals: {e}");
    }
    if args.list {
        return list(out);
    }
    let built = if args.idea {
        idea(&paths, args.name.as_deref(), &words, &footer)
    } else {
        let Some(name) = &args.name else {
            return out.error(Exit::Usage, "name the rule to share, or pass --idea");
        };
        match share::check_rule(&paths, name, now) {
            Ok((evidence, why, code)) => Ok((
                share::rule_proposal(name, &why, &code, &evidence, &words, &footer),
                Vec::new(),
            )),
            Err(problems) if problems.len() == 1 && problems[0].starts_with("no rule") => {
                return out.error(Exit::NotFound, &problems[0]);
            }
            Err(problems) => Err(format!(
                "`{name}` is not ready to share:\n- {}",
                problems.join("\n- ")
            )),
        }
    };
    let (proposal, keys) = match built {
        Ok(built) => built,
        Err(message) => return out.error(Exit::Precondition, &message),
    };
    if let Some(secret) =
        crate::redact::find_secret(&format!("{}\n{}", proposal.title, proposal.body))
    {
        return out.error(
            Exit::Precondition,
            &format!("refusing to share: the proposal still holds what looks like a secret ({secret}); remove it from the rule"),
        );
    }
    if !args.yes {
        let report = Report {
            proposal: &proposal,
            posted: false,
            url: None,
        };
        out.result(&report, || preview(&proposal, args));
        return Exit::Ok;
    }
    let Some(posted_path) = share::posted_path() else {
        return out.error(
            Exit::Failed,
            "there is no home directory to record posts in",
        );
    };
    let recent = share::posted_this_week(&posted_path, now);
    if recent >= share::MAX_POSTS {
        return out.error(
            Exit::Precondition,
            &format!("{recent} proposals were posted in the last week, the most jouet posts; try again later"),
        );
    }
    let mut client = jouet_github::Client::from_env();
    let (login, url) = match share::post(&mut client, &proposal) {
        Ok(posted) => posted,
        Err(Refusal::NoCategory)
            if proposal.destination == share::signals::Destination::Discussion =>
        {
            return out.error(
                Exit::Precondition,
                &format!(
                    "{}/{} has no \"{}\" Discussions category yet, so nothing was posted; the maintainers open it when they are ready for proposals",
                    share::OWNER,
                    share::REPO,
                    share::CATEGORY
                ),
            );
        }
        Err(Refusal::Duplicate(url)) => {
            return out.error(
                Exit::Precondition,
                &format!("a proposal with this title already exists, so nothing was posted: {url}"),
            );
        }
        Err(Refusal::Remote(message)) => return out.error(Exit::Remote, &message),
        Err(Refusal::NoCategory) => {
            return out.error(Exit::Remote, "no Discussions category for an Issue");
        }
    };
    let record = Posted {
        ts: now,
        kind: proposal.kind.into(),
        title: proposal.title.clone(),
        url: url.clone(),
    };
    if let Err(e) = share::note_posted(&posted_path, &record) {
        eprintln!("jouet: recording the post: {e}");
    }
    if proposal.kind == "rule"
        && let Some(name) = &args.name
    {
        let change = rulefile::Change {
            ts: now,
            rule: name.clone(),
            change: "shared".into(),
            from: None,
            to: Some(url.clone()),
            reason: "proposed to jouet's maintainers".into(),
            by: login.clone(),
        };
        if let Err(e) = rulefile::log(&paths.log, &change) {
            eprintln!("jouet: writing the rule log: {e}");
        }
    }
    if !keys.is_empty()
        && let Some(path) = share::gaps_path()
    {
        let _ = share::update_gaps(&path, |gaps| {
            for gap in gaps.gaps.iter_mut().filter(|g| keys.contains(&g.key())) {
                gap.shared = true;
            }
        });
    }
    let report = Report {
        proposal: &proposal,
        posted: true,
        url: Some(url.clone()),
    };
    out.result(&report, || format!("posted as {login}: {url}\n"));
    Exit::Ok
}

/// The idea proposal for a key, or the most recorded one.
fn idea(
    paths: &Paths,
    key: Option<&str>,
    words: &[String],
    footer: &str,
) -> Result<(Proposal, Vec<String>), String> {
    let gaps = share::gaps_path()
        .map(|p| share::read_gaps(&p))
        .unwrap_or_default();
    let ideas = share::ideas(&gaps);
    if ideas.is_empty() {
        return Err("no ideas are recorded: jouet records one when a rule reaches for a primitive the API lacks, or when sessions show jouet crashing, misbehaving or falling short; `jouet rule gap TEXT` records one in your words".into());
    }
    let (key, entries) = match key {
        Some(key) => ideas.iter().find(|(k, _)| k == key).ok_or_else(|| {
            let keys: Vec<&str> = ideas.iter().map(|(k, _)| k.as_str()).collect();
            format!("no recorded gap `{key}`; recorded: {}", keys.join(", "))
        })?,
        None => &ideas[0],
    };
    if let Some(signal) = entries.iter().find_map(|g| g.signal.as_ref()) {
        return Ok((
            share::signal_proposal(signal, words, footer),
            vec![key.clone()],
        ));
    }
    // What the rules that reached for it were trying to do.
    let engine = wind::user_engine(&paths.rules);
    let why: Vec<String> = entries
        .iter()
        .filter_map(|g| g.rule.as_deref())
        .filter_map(|r| engine.rule(r).map(|rule| rule.why.clone()))
        .filter(|w| !w.is_empty())
        .collect();
    Ok((
        share::idea_proposal(key, entries, &why, words, footer),
        vec![key.clone()],
    ))
}

#[derive(Serialize)]
struct Idea {
    key: String,
    destination: share::signals::Destination,
    summary: String,
}

/// `jouet share --list`: the ideas ready to share.
fn list(out: &Output) -> Exit {
    let gaps = share::gaps_path()
        .map(|p| share::read_gaps(&p))
        .unwrap_or_default();
    let ideas: Vec<Idea> = share::ideas(&gaps)
        .into_iter()
        .map(|(key, entries)| {
            let signal = entries.iter().find_map(|g| g.signal.as_ref());
            Idea {
                destination: signal
                    .map_or(share::signals::Destination::Discussion, |s| s.destination()),
                summary: match signal {
                    Some(s) => s.summary(),
                    None => {
                        let count: u64 = entries.iter().map(|g| g.count).sum();
                        let what = entries
                            .iter()
                            .find_map(|g| {
                                g.primitive
                                    .clone()
                                    .map(|p| format!("`jouet.{p}`, which the rule API lacks"))
                            })
                            .or_else(|| entries.iter().find_map(|g| g.note.clone()))
                            .unwrap_or_default();
                        format!("{what} ({count} times)")
                    }
                },
                key,
            }
        })
        .collect();
    out.result(&serde_json::json!({ "ideas": ideas }), || {
        if ideas.is_empty() {
            return "no ideas are ready to share\n".into();
        }
        let mut text = String::new();
        for idea in &ideas {
            let place = match idea.destination {
                share::signals::Destination::Issue => "issue     ",
                share::signals::Destination::Discussion => "discussion",
            };
            let _ = writeln!(text, "{}  {place}  {}", idea.key, idea.summary);
        }
        text.push_str("`jouet share --idea KEY` drafts one; nothing is posted without --yes\n");
        text
    });
    Exit::Ok
}

fn preview(proposal: &Proposal, args: &ShareArgs) -> String {
    let mut text = format!("title: {}\n\n{}", proposal.title, proposal.body);
    if proposal.diff.is_empty() {
        text.push_str("\nanonymizing changed nothing.\n");
    } else {
        let _ = write!(text, "\nwhat anonymizing changed:\n{}", proposal.diff);
    }
    let mut again = String::from("jouet share");
    if let Some(name) = &args.name {
        let _ = write!(again, " {name}");
    }
    if args.idea {
        again.push_str(" --idea");
    }
    for word in &args.redact {
        let _ = write!(again, " --redact {word}");
    }
    let place = match proposal.destination {
        share::signals::Destination::Issue => "as an Issue".to_string(),
        share::signals::Destination::Discussion => format!("to \"{}\"", share::CATEGORY),
    };
    let _ = writeln!(
        text,
        "\nNothing was posted. Show this to the user; run `{again} --yes` only once they agree, to post it {place} on {}/{} with their GitHub login. `--redact WORD` hides more.",
        share::OWNER,
        share::REPO
    );
    text
}
