//! `jouet pr next`: the built-in `pr-next` routine, run with the same
//! arguments. The routine selects and fetches PRs the way `pr status` does
//! (`github.prs`), then decides, prints and picks the exit code in Luau.
//! Measured against the Rust version it replaced, the run is within noise:
//! the wall time is GitHub's.

use super::Output;
use crate::NextArgs;
use jouet_output::exit::Exit;
use jouet_routines::routine::{self, Catalog};
use std::process::ExitCode;

pub(crate) fn run(args: &NextArgs, out: &Output) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            return out
                .error(Exit::Failed, &format!("reading the current directory: {e}"))
                .into();
        }
    };
    let catalog = Catalog::new(routine::builtin());
    let Some(routine) = catalog.get("pr-next") else {
        return out
            .error(
                Exit::Failed,
                "the pr-next routine is missing from this jouet",
            )
            .into();
    };
    super::run::run_routine(&catalog, routine, &words(args), &cwd, out)
}

/// The routine's words for the parsed arguments: flags, then `--` and the
/// PRs, so a branch named like a flag stays a PR.
fn words(args: &NextArgs) -> Vec<String> {
    let mut words = Vec::new();
    if args.mine {
        words.push("--mine".to_string());
    }
    if let Some(query) = &args.search {
        words.push(format!("--search={query}"));
    }
    for repo in &args.repo {
        words.push(format!("--repo={repo}"));
    }
    if let Some(settle) = &args.settle {
        words.push(format!("--settle={settle}"));
    }
    if !args.prs.is_empty() {
        words.push("--".into());
        words.extend(args.prs.iter().cloned());
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_pass_to_the_routine_as_written() {
        let args = NextArgs {
            prs: vec!["7".into(), "-odd-branch".into()],
            mine: true,
            search: Some("label:bug".into()),
            repo: vec!["jdx/a".into(), "jdx/b".into()],
            settle: Some("0".into()),
        };
        assert_eq!(
            words(&args),
            [
                "--mine",
                "--search=label:bug",
                "--repo=jdx/a",
                "--repo=jdx/b",
                "--settle=0",
                "--",
                "7",
                "-odd-branch"
            ]
        );
        let catalog = Catalog::new(routine::builtin());
        let parsed = catalog
            .get("pr-next")
            .unwrap()
            .parse_args(&words(&args))
            .unwrap();
        assert_eq!(
            parsed.values["prs"],
            serde_json::json!(["7", "-odd-branch"])
        );
        assert_eq!(parsed.values["repo"], serde_json::json!(["jdx/a", "jdx/b"]));
    }
}
