//! `jouet claim`: advisory claims between agent sessions on one machine.
//! The store is jouet-git's; the hooks' `claimed-path` rule reads it.

use super::Output;
use crate::{ClaimArgs, ClaimCmd, ClaimLsArgs, ClaimReleaseArgs};
use jouet_git::claim::{Claim, Conflict, Holder, Refused, Standing, Store};
use jouet_output::exit::Exit;
use jouet_output::time;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_FOR: Duration = Duration::from_secs(2 * 3600);

pub(crate) fn run(args: ClaimArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let Some(state) = jouet_output::paths::state_dir().map(|d| d.join("claims")) else {
        return out.error(
            Exit::Precondition,
            "no state directory (set HOME or XDG_STATE_HOME)",
        );
    };
    let Some(store) = Store::open(&state, &cwd) else {
        return out.error(Exit::Precondition, "not in a git work tree");
    };
    let now = time::now();
    match args.cmd {
        Some(ClaimCmd::Ls(a)) => ls(&a, &store, now, out),
        Some(ClaimCmd::Release(a)) => release(&a, &store, &cwd, now, out),
        None => claim(&args, &store, &cwd, now, out),
    }
}

/// The session claiming, or the error that says there is none.
fn holder(out: &Output) -> Result<Holder, Exit> {
    Holder::from_env().ok_or_else(|| {
        out.error_with_hint(
            Exit::Precondition,
            "no agent session: claims belong to a Claude Code or Codex session",
            Some("run it from the agent's shell, or set JOUET_SESSION_ID"),
        )
    })
}

/// A claim another session holds, as the conflict lists it.
#[derive(Serialize)]
struct Held<'a> {
    /// The pattern asked for.
    wanted: &'a str,
    claim: &'a Claim,
}

fn claim(args: &ClaimArgs, store: &Store, cwd: &Path, now: i64, out: &Output) -> Exit {
    if args.paths.is_empty() {
        return out.error(
            Exit::Usage,
            "name the paths to claim, as in `jouet claim src/cmd/ --note 'reworking the commands'`",
        );
    }
    let holder = match holder(out) {
        Ok(h) => h,
        Err(exit) => return exit,
    };
    let duration = match args.duration.as_deref().map(time::parse_duration) {
        None => DEFAULT_FOR,
        Some(Ok(d)) if !d.is_zero() => d,
        Some(Ok(_)) => return out.error(Exit::Usage, "--for must be longer than 0s"),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--for: {e}")),
    };
    let mut patterns = Vec::new();
    for arg in &args.paths {
        match store.pattern(cwd, arg) {
            Ok(p) if !patterns.contains(&p) => patterns.push(p),
            Ok(_) => {}
            Err(message) => return out.error(Exit::Usage, &message),
        }
    }
    store.prune(now);
    let expires = now + i64::try_from(duration.as_secs()).unwrap_or(i64::MAX / 2);
    match store.claim(&holder, &patterns, expires, args.note.as_deref(), now) {
        Ok(claims) => {
            out.result(&serde_json::json!({ "claimed": claims }), || {
                format!(
                    "CLAIMED {} for {}, until {} ({})\n  other sessions are warned when they edit or commit these; `jouet claim release` gives them up\n",
                    patterns.join(", "),
                    claims[0].who(),
                    claims[0].expires_at,
                    time::age(expires - now),
                )
            });
            Exit::Ok
        }
        Err(Refused::Io(message)) => out.error(Exit::Failed, &message),
        Err(Refused::Conflicts(conflicts)) => {
            refused(&conflicts, now, out);
            Exit::Failed
        }
    }
}

fn refused(conflicts: &[Conflict], now: i64, out: &Output) {
    let held: Vec<Held> = conflicts
        .iter()
        .map(|c| Held {
            wanted: &c.wanted,
            claim: &c.held,
        })
        .collect();
    out.result(&serde_json::json!({ "conflicts": held }), || {
        let mut s = String::from("REFUSED other sessions claim overlapping paths; nothing was claimed\n");
        for c in conflicts {
            let _ = writeln!(s, "  {}: {}", c.wanted, describe(&c.held, now));
        }
        s.push_str("  next: work elsewhere, or coordinate with that session; its claim lapses when it expires or the session ends\n");
        s
    });
}

/// Who holds a claim and where, as one line: the claim rule's wording too.
pub(crate) fn describe(claim: &Claim, now: i64) -> String {
    let mut s = format!(
        "claimed as {} by {} in {}",
        claim.pattern,
        claim.who(),
        tilde(&claim.worktree)
    );
    if let Some(branch) = &claim.branch {
        let _ = write!(s, " (branch {branch})");
    }
    if let Some(t) = time::parse_utc(&claim.expires_at) {
        let _ = write!(s, ", {} left", time::age(t - now));
    }
    if let Some(note) = &claim.note {
        let _ = write!(s, ": {note}");
    }
    s
}

/// One row of `claim ls`.
#[derive(Serialize)]
struct Row<'a> {
    #[serde(flatten)]
    claim: &'a Claim,
    /// The claim is this session's.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    mine: bool,
    /// Why it no longer holds; absent while it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    lapsed: Option<&'static str>,
}

fn ls(args: &ClaimLsArgs, store: &Store, now: i64, out: &Output) -> Exit {
    let session = Holder::from_env().map(|h| h.session);
    let claims: Vec<(Claim, Standing)> = if args.all {
        store.all(now)
    } else {
        store.prune(now);
        store
            .live(now)
            .into_iter()
            .map(|c| (c, Standing::Live))
            .collect()
    };
    let rows: Vec<Row> = claims
        .iter()
        .map(|(c, standing)| Row {
            claim: c,
            mine: session.as_deref() == Some(c.session.as_str()),
            lapsed: (*standing != Standing::Live).then(|| standing.name()),
        })
        .collect();
    out.result(&rows, || {
        if rows.is_empty() {
            return "no claims in this repository; `jouet claim PATH` claims one\n".into();
        }
        let mut s = String::new();
        for r in &rows {
            let _ = write!(s, "{}", describe(r.claim, now));
            if r.mine {
                s.push_str(" (yours)");
            }
            if let Some(why) = r.lapsed {
                let _ = write!(s, " [{why}]");
            }
            s.push('\n');
        }
        s
    });
    Exit::Ok
}

fn release(args: &ClaimReleaseArgs, store: &Store, cwd: &Path, now: i64, out: &Output) -> Exit {
    let holder = match holder(out) {
        Ok(h) => h,
        Err(exit) => return exit,
    };
    let mut patterns = Vec::new();
    for arg in &args.paths {
        match store.pattern(cwd, arg) {
            Ok(p) => patterns.push(p),
            Err(message) => return out.error(Exit::Usage, &message),
        }
    }
    let released = store.release(&holder.session, &patterns, now);
    if released.is_empty() && !patterns.is_empty() {
        return out.error_with_hint(
            Exit::NotFound,
            &format!("this session claims none of {}", patterns.join(", ")),
            Some("`jouet claim ls` lists the claims"),
        );
    }
    out.result(&serde_json::json!({ "released": released }), || {
        if released.is_empty() {
            return "this session holds no claims in this repository\n".into();
        }
        let names: Vec<&str> = released.iter().map(|c| c.pattern.as_str()).collect();
        format!("RELEASED {}\n", names.join(", "))
    });
    Exit::Ok
}

/// A path with the home directory written as `~`.
fn tilde(path: &str) -> String {
    let home: Option<PathBuf> = std::env::home_dir();
    match home.and_then(|h| Path::new(path).strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.to_string(),
    }
}
