//! `jouet run`: recipes, and the host that runs their steps by running this
//! binary again.

use super::Output;
use crate::RunArgs;
use crate::affected::{self, task};
use crate::rules;
use jouet_output::exit::Exit;
use jouet_rules::Trust;
use jouet_rules::recipe::{self, Host, Recipe, Recipes, Report, State, Step, StepOutput};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Lines of a test run's output kept in its result.
const TAIL_LINES: usize = 30;

pub(crate) fn run(args: &RunArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    if args.trust || args.untrust {
        return trust(&cwd, args.trust, out);
    }
    let store = rules::trust_store();
    let repo_dir = rules::repo_recipe_dir(&cwd);
    let (sources, errors) = recipe::discover(
        rules::user_recipe_dir().as_deref(),
        repo_dir.as_deref(),
        &store,
    );
    let recipes = match Recipes::new(
        sources,
        recipe::Options {
            has_command: Box::new(crate::spec::has_command),
            cache_dir: jouet_output::paths::cache_dir().map(|d| d.join("luau")),
        },
    ) {
        Ok(recipes) => recipes,
        Err(e) => return out.error(Exit::Failed, &format!("starting the recipe engine: {e}")),
    };
    let mut notes = errors;
    if let Some(dir) = &repo_dir {
        match store.check(dir) {
            Trust::Trusted => {}
            Trust::Untrusted => notes.push(format!(
                "{} is not trusted, so its recipes do not run; review them, then run `jouet run --trust`",
                dir.display()
            )),
            Trust::Changed => notes.push(format!(
                "{} changed since it was trusted, so its recipes do not run; review them, then run `jouet run --trust`",
                dir.display()
            )),
        }
    }
    let name = match &args.recipe {
        Some(name) if !args.list => name,
        _ => return list(&recipes, &notes, out),
    };
    let Some(recipe) = recipes.get(name) else {
        let names: Vec<&str> = recipes.recipes().iter().map(|r| r.name.as_str()).collect();
        return out.error_with_hint(
            Exit::NotFound,
            &format!("no recipe `{name}`"),
            Some(&format!(
                "the recipes are {}; see `jouet run --list`",
                names.join(", ")
            )),
        );
    };
    let host = Jouet {
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("jouet")),
        root: repo_root(&cwd),
    };
    let report = recipes.run(
        recipe,
        &args.args,
        rules::probes(&cwd, true),
        Box::new(host),
        args.dry_run,
    );
    out.result(&report, || render(&report));
    if !out.json
        && let Some(error) = &report.error
    {
        eprintln!("jouet: recipe {}: {error}", report.recipe);
    }
    Exit::from_code(report.exit_code).unwrap_or(Exit::Failed)
}

#[derive(Serialize)]
struct Listed<'a> {
    name: &'a str,
    state: &'a State,
    source: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    why: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<&'a str>,
}

#[derive(Serialize)]
struct List<'a> {
    recipes: Vec<Listed<'a>>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    notes: &'a [String],
}

fn list(recipes: &Recipes, notes: &[String], out: &Output) -> Exit {
    let listed = List {
        recipes: recipes.recipes().iter().map(listed).collect(),
        notes,
    };
    out.result(&listed, || {
        let width = listed
            .recipes
            .iter()
            .map(|r| r.name.len())
            .max()
            .unwrap_or(0);
        let mut s = String::new();
        for r in &listed.recipes {
            let state = match r.state {
                State::Ready => String::new(),
                State::Untrusted => " (untrusted)".into(),
                State::Broken(e) => format!(" (broken: {e})"),
            };
            let _ = writeln!(s, "{:<width$}  {}{state}", r.name, r.why);
            if let Some(usage) = r.usage {
                let _ = writeln!(s, "{:<width$}  args: {usage}", "");
            }
            if !matches!(r.source.as_str(), "builtin") {
                let _ = writeln!(s, "{:<width$}  from {}", "", r.source);
            }
        }
        for note in notes {
            let _ = writeln!(s, "note: {note}");
        }
        s
    });
    Exit::Ok
}

fn listed(recipe: &Recipe) -> Listed<'_> {
    Listed {
        name: &recipe.name,
        state: &recipe.state,
        source: recipe.origin.describe(),
        why: &recipe.why,
        usage: recipe.usage.as_deref(),
    }
}

/// A dry run's steps, or the recipe's message.
fn render(report: &Report) -> String {
    let mut s = String::new();
    if report.dry_run {
        let _ = writeln!(s, "{} would run:", report.recipe);
        for (i, step) in report.steps.iter().enumerate() {
            let _ = writeln!(s, "  {}. {}", i + 1, step.command);
        }
        if report.steps.is_empty() {
            let _ = writeln!(s, "  nothing");
        }
    }
    // A dry run's message is about placeholder results, so it says nothing.
    if report.dry_run {
        return s;
    }
    if let Some(message) = &report.message {
        let _ = writeln!(s, "{message}");
    } else if report.error.is_none() {
        let _ = writeln!(s, "{}: done", report.recipe);
    }
    s
}

#[derive(Serialize)]
struct Trusted {
    dir: String,
    trusted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipes: Option<usize>,
}

fn trust(cwd: &Path, trust: bool, out: &Output) -> Exit {
    let Some(dir) = rules::repo_recipe_dir(cwd) else {
        return out.error(
            Exit::NotFound,
            "no .jouet/recipes directory at the root of this repository",
        );
    };
    let mut store = rules::trust_store();
    let result = if trust {
        store.trust(&dir).map(|()| true)
    } else {
        store.untrust(&dir)
    };
    let changed = match result {
        Ok(changed) => changed,
        Err(e) => {
            return out.error(
                Exit::Failed,
                &format!("updating trust for {}: {e}", dir.display()),
            );
        }
    };
    let count = jouet_rules::rule_files(&dir).map(|f| f.len()).ok();
    let report = Trusted {
        dir: dir.display().to_string(),
        trusted: trust,
        recipes: if trust { count } else { None },
    };
    out.result(&report, || match (trust, changed) {
        (true, _) => format!(
            "trusted {} ({} recipes); a change to any of them needs trusting again\n",
            report.dir,
            count.unwrap_or(0)
        ),
        (false, true) => format!("no longer trusting {}\n", report.dir),
        (false, false) => format!("{} was not trusted\n", report.dir),
    });
    Exit::Ok
}

/// The top of the git work tree `cwd` is in, else `cwd`.
fn repo_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// Runs a recipe's steps: jouet's commands by running this binary with
/// `--json`, and the test task in the repository's root.
struct Jouet {
    exe: PathBuf,
    root: PathBuf,
}

impl Host for Jouet {
    fn describe(&self, step: &Step) -> String {
        match step.argv() {
            Some(argv) => {
                let words: Vec<String> = argv[1..].iter().map(|w| affected::quote(w)).collect();
                format!("jouet {}", words.join(" "))
            }
            None => match task::test_task(&self.root) {
                Some(task) => task
                    .iter()
                    .map(|w| affected::quote(w))
                    .collect::<Vec<_>>()
                    .join(" "),
                None => "the test task (none found)".into(),
            },
        }
    }

    fn run(&mut self, step: &Step) -> Result<StepOutput, String> {
        eprintln!("jouet run: {}", self.describe(step));
        let Some(argv) = step.argv() else {
            return self.test();
        };
        let output = Command::new(&self.exe)
            .args(&argv)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("running {}: {e}", self.exe.display()))?;
        Ok(StepOutput {
            exit_code: output.status.code().unwrap_or(1),
            json: serde_json::from_slice(&output.stdout).ok(),
        })
    }
}

impl Jouet {
    /// Runs the test task with its output in a log, and reports how it
    /// ended with the end of the log.
    fn test(&self) -> Result<StepOutput, String> {
        let task = task::test_task(&self.root).ok_or_else(|| {
            format!(
                "no test task found in {}: looked for a mise `test` task, a package.json `test` script, Cargo.toml, a justfile or Makefile `test` target, and go.mod",
                self.root.display()
            )
        })?;
        let log = log_path();
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        let file =
            std::fs::File::create(&log).map_err(|e| format!("creating {}: {e}", log.display()))?;
        let err = file.try_clone().map_err(|e| e.to_string())?;
        let started = Instant::now();
        let status = Command::new(&task[0])
            .args(&task[1..])
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(err)
            .status()
            .map_err(|e| format!("running {}: {e}", task[0]))?;
        let exit_code = status.code().unwrap_or(1);
        Ok(StepOutput {
            exit_code,
            json: Some(serde_json::json!({
                "command": task,
                "exit_code": exit_code,
                "duration_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "log": log.display().to_string(),
                "tail": tail(&log, TAIL_LINES),
            })),
        })
    }
}

fn log_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    jouet_output::paths::log_dir().join(format!("run-test-{stamp}-{}.log", std::process::id()))
}

/// The last `n` lines of a file.
fn tail(path: &Path, n: usize) -> Vec<String> {
    let mut text = String::new();
    let _ = std::fs::File::open(path).and_then(|mut f| f.read_to_string(&mut text));
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| (*l).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_a_recipe_can_run_is_in_this_jouet() {
        for command in recipe::COMMANDS.iter().filter(|c| !c.path.is_empty()) {
            assert!(crate::spec::has_command(command.path), "{}", command.name);
        }
    }
}
