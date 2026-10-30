//! `jouet.pr.status()` and `jouet.pr.next()`: one PR, fetched the way
//! `jouet pr status` fetches it, through jouet-github.
//!
//! Unlike the command, a probe does not wait for GitHub to settle an
//! unknown mergeable state: it answers with what GitHub says now.

use super::PrView;
use jouet_github::Client;
use jouet_github::pr::next;
use jouet_github::pr::status::{self, Lookup};
use jouet_github::pr::target::{self, Arg, Target};

/// Fetches the PR `arg` names (a number, URL, `OWNER/REPO#N` or branch), or
/// the current branch's PR. `Ok(None)` when there is no such PR.
pub(crate) fn fetch(arg: Option<&str>) -> Result<Option<PrView>, String> {
    let repo = || {
        target::repo_from_git()
            .ok_or_else(|| "no GitHub remote here, so name the PR as OWNER/REPO#N".to_string())
    };
    let target = match arg {
        None => {
            let branch = target::current_branch().ok_or("not on a branch: name a PR")?;
            Target::Branch(repo()?, branch)
        }
        Some(arg) => match Arg::parse(arg) {
            Arg::Number(n) => Target::Number(repo()?, n),
            Arg::Branch(branch) => Target::Branch(repo()?, branch),
            Arg::InRepo(repo, n) => Target::Number(repo, n),
        },
    };
    let fetched = status::fetch(&mut Client::from_env(), vec![Lookup::Pr(target)])?;
    let Some(pr) = fetched.prs.into_iter().next() else {
        return Ok(None);
    };
    let pr = pr.finish(jouet_output::time::now());
    let mut next_view = serde_json::to_value(next::next(&pr)).map_err(|e| e.to_string())?;
    for (key, value) in [
        ("repo", pr.repo.as_str().into()),
        ("number", pr.number.into()),
        ("title", pr.title.as_str().into()),
        ("url", pr.url.as_str().into()),
    ] {
        next_view[key] = value;
    }
    Ok(Some(PrView {
        status: serde_json::to_value(&pr).map_err(|e| e.to_string())?,
        next: next_view,
    }))
}
