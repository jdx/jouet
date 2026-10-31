//! Adding what Entire says to PRs that `pr status` and `pr next` fetched.
//!
//! The gates come with the PR, from its Entire Gates check, and say why each
//! one fails; an out-of-date gate already says how far behind the branch is.
//! Only an open PR whose findings gate fails is worth asking the `entire`
//! CLI about, for the findings themselves, so the CLI is not even started
//! for a repository without Entire. So is one whose check failed with a
//! summary jouet cannot read: its gates are taken from `entire trail show`
//! instead, and it says so, rather than guessed at. The PRs that need it are
//! asked in parallel.

use super::status::{Fetched, PrStatus, State};
use crate::entire::{Availability, Brief, Entire};

fn wanted(pr: &PrStatus) -> bool {
    matches!(pr.state, State::Open | State::Draft)
        && pr
            .entire
            .as_ref()
            .is_some_and(|e| e.gates.fails("findings") || !e.gates.read())
}

/// What a PR says when its gates came from the trail.
const FROM_TRAIL: &str = "jouet could not read the Entire Gates check's summary, so its gates come from `entire trail show`";

/// Fills in the gates of each PR whose check summary jouet cannot read, and
/// the open findings of each whose findings gate fails.
/// `None` when no PR did, so Entire was not asked.
pub(crate) fn add(prs: &mut [Fetched]) -> Option<Availability> {
    if !prs.iter_mut().any(|p| wanted(p.status_mut())) {
        return None;
    }
    let probed = Entire::probe();
    let availability = Availability::of(&probed);
    let Ok(entire) = probed else {
        return Some(availability);
    };
    std::thread::scope(|s| {
        for pr in prs
            .iter_mut()
            .map(Fetched::status_mut)
            .filter(|p| wanted(p))
        {
            let entire = &entire;
            s.spawn(move || fill(entire, pr));
        }
    });
    Some(availability)
}

fn fill(entire: &Entire, pr: &mut PrStatus) {
    // With its host, so a PR off github.com is not looked up as `gh/...`.
    let Some(repo) = pr.repository() else {
        return;
    };
    let name = format!("{}#{}", pr.repo, pr.number);
    let head = pr.head.clone();
    let Some(status) = pr.entire.as_mut() else {
        return;
    };
    let mut read = || -> Result<(), String> {
        let Some(trail) = entire.trail(&repo, status.gates.trail, &head)? else {
            return Ok(());
        };
        if !status.gates.read() && !trail.gates.is_empty() {
            status.gates.gates = trail.gates.clone();
            status.gates.trail = Some(trail.number);
            status.note = Some(FROM_TRAIL.into());
        }
        if !status.gates.fails("findings") {
            return Ok(());
        }
        let found = entire.findings(&repo, trail.number, false)?;
        let open: Vec<_> = found.findings.into_iter().filter(|f| f.is_open()).collect();
        status.findings = Brief::of(&open);
        status.findings_incomplete = found.incomplete;
        Ok(())
    };
    if let Err(e) = read() {
        eprintln!("jouet: entire could not describe {name}: {e}");
        status.error = Some(e);
    }
}
