//! `jouet run`, and `joue`, which is a view of it: routines, and the host
//! that runs jouet's own commands for them by running this binary again.

use super::Output;
use crate::rules;
use crate::{RunArgs, RunCmd, RunTestArgs};
use jouet_output::exit::Exit;
use jouet_routines::routine::{self, Catalog, Routine, State};
use jouet_routines::world::Github;
use jouet_routines::{ArgError, Host, Options, StepOutput, World, testing};
use jouet_rules::Trust;
use serde::Serialize;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

pub(crate) fn run(args: RunArgs, out: &Output) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            return out
                .error(Exit::Failed, &format!("reading the current directory: {e}"))
                .into();
        }
    };
    if args.trust || args.untrust {
        return trust(&cwd, args.trust, out).into();
    }
    let (catalog, notes) = catalog(&cwd);
    match args.cmd {
        Some(RunCmd::External(words)) if !args.list => run_words(&catalog, &cwd, &words, out),
        Some(RunCmd::Test(test)) => run_tests(&catalog, &test, out).into(),
        Some(RunCmd::Types(_)) => {
            print!("{}", jouet_routines::TYPES);
            ExitCode::SUCCESS
        }
        _ => {
            let mut catalog = catalog;
            catalog.check();
            list(&catalog, &notes, out).into()
        }
    }
}

/// Every routine, and notes on what could not be read or is not trusted.
fn catalog(cwd: &Path) -> (Catalog, Vec<String>) {
    let store = rules::trust_store();
    let repo_dir = rules::repo_routine_dir(cwd);
    let trust = repo_dir.as_deref().map(|dir| store.check(dir));
    let (sources, mut notes) = routine::discover(
        rules::user_routine_dir().as_deref(),
        repo_dir.as_deref(),
        trust == Some(Trust::Trusted),
    );
    if let (Some(dir), Some(trust)) = (&repo_dir, trust) {
        let why = match trust {
            Trust::Trusted => None,
            Trust::Untrusted => Some("is not trusted"),
            Trust::Changed => Some("changed since it was trusted"),
        };
        if let Some(why) = why {
            notes.push(format!(
                "{} {why}, so its routines do not run; review them, then run `joue --trust`",
                dir.display()
            ));
        }
    }
    (Catalog::new(sources), notes)
}

/// Whether `word` names a routine, for `jouet`'s "did you mean `joue`".
pub(crate) fn is_routine(word: &str) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    catalog(&cwd).0.get(word).is_some()
}

/// Runs the routine the first word names, with the rest as its arguments.
fn run_words(catalog: &Catalog, cwd: &Path, words: &[OsString], out: &Output) -> ExitCode {
    let words: Vec<String> = words
        .iter()
        .map(|w| w.to_string_lossy().into_owned())
        .collect();
    let Some((name, rest)) = words.split_first() else {
        return list(catalog, &[], out).into();
    };
    let Some(routine) = catalog.get(name) else {
        return unknown(catalog, &words, out).into();
    };
    run_routine(catalog, routine, rest, cwd, out)
}

/// Runs a routine and prints what it came to.
pub(crate) fn run_routine(
    catalog: &Catalog,
    routine: &Routine,
    words: &[String],
    cwd: &Path,
    out: &Output,
) -> ExitCode {
    let args = match routine.parse_args(words) {
        Ok(args) => args,
        Err(ArgError::Help(page)) => {
            print!("{page}");
            if !page.ends_with('\n') {
                println!();
            }
            return ExitCode::SUCCESS;
        }
        Err(ArgError::Usage(e)) => {
            return out
                .error(Exit::Usage, &format!("{}: {e}", routine.name))
                .into();
        }
    };
    let options = Options {
        json: out.json || args.json,
        yes: args.yes,
        dry_run: args.dry_run,
        testing: false,
    };
    let world = World {
        root: cwd.to_path_buf(),
        host: Box::new(Jouet {
            exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("jouet")),
        }),
        github: Github::Env,
        ledger: crate::ledger::dir(),
        repo_key: crate::ledger::repo_key(cwd),
        log_dir: jouet_output::paths::log_dir(),
        recorded: None,
    };
    let outcome = jouet_routines::runtime::run(catalog, routine, args, world, options);
    let out = Output {
        json: options.json,
        mode: out.mode,
    };
    if let Some(fail) = &outcome.error {
        if !out.json {
            print!("{}", outcome.text);
        }
        out.error_with_hint(fail.exit, &fail.message, fail.hint.as_deref());
    } else {
        out.result(&testing::document(&outcome), || {
            let mut text = outcome.text.clone();
            if !outcome.planned.is_empty() {
                let _ = writeln!(text, "not done, without --yes:");
                for write in &outcome.planned {
                    let _ = writeln!(text, "  {write}");
                }
            }
            text
        });
        if outcome.needs_yes {
            eprintln!(
                "jouet: {} would make {} change{}; run it again with --yes to make {}, or --dry-run to only look",
                routine.name,
                outcome.planned.len(),
                if outcome.planned.len() == 1 { "" } else { "s" },
                if outcome.planned.len() == 1 {
                    "it"
                } else {
                    "them"
                },
            );
        }
    }
    ExitCode::from(u8::try_from(outcome.exit_code).unwrap_or(1))
}

/// No routine by that name: suggest a jouet command or a routine close to it.
fn unknown(catalog: &Catalog, words: &[String], out: &Output) -> Exit {
    let name = &words[0];
    let as_command: Vec<&str> = words.iter().take(2).map(String::as_str).collect();
    let hint = if crate::spec::has_command(&as_command[..1]) {
        let shown = if crate::spec::has_command(&as_command) {
            as_command.join(" ")
        } else {
            name.clone()
        };
        Some(format!(
            "did you mean `jouet {shown}`? It is one of jouet's own commands"
        ))
    } else {
        match catalog.similar(name).as_slice() {
            [] => Some("see `joue --list`".to_string()),
            close => Some(format!(
                "did you mean {}?",
                close
                    .iter()
                    .map(|c| format!("`joue {c}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )),
        }
    };
    out.error_with_hint(
        Exit::NotFound,
        &format!("no routine `{name}`"),
        hint.as_deref(),
    )
}

#[derive(Serialize)]
struct Listed<'a> {
    name: &'a str,
    state: &'a State,
    source: String,
    about: &'a str,
    capabilities: &'a [jouet_routines::Capability],
}

#[derive(Serialize)]
struct List<'a> {
    routines: Vec<Listed<'a>>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    notes: &'a [String],
}

fn list(catalog: &Catalog, notes: &[String], out: &Output) -> Exit {
    let listed = List {
        routines: catalog
            .routines()
            .iter()
            .map(|r| Listed {
                name: &r.name,
                state: &r.state,
                source: r.origin.describe(),
                about: &r.about,
                capabilities: &r.capabilities,
            })
            .collect(),
        notes,
    };
    out.result(&listed, || {
        let width = listed
            .routines
            .iter()
            .map(|r| r.name.len())
            .max()
            .unwrap_or(0);
        let mut s = String::new();
        for r in &listed.routines {
            let state = match r.state {
                State::Ready => String::new(),
                State::Untrusted => " (untrusted)".into(),
                State::Broken(e) => format!(" (broken: {e})"),
            };
            let _ = writeln!(s, "{:<width$}  {}{state}", r.name, r.about);
            if !r.capabilities.is_empty() {
                let caps: Vec<&str> = r.capabilities.iter().map(|c| c.name()).collect();
                let _ = writeln!(s, "{:<width$}  reaches {}", "", caps.join(", "));
            }
            if r.source != "builtin" {
                let _ = writeln!(s, "{:<width$}  from {}", "", r.source);
            }
        }
        for note in notes {
            let _ = writeln!(s, "note: {note}");
        }
        let _ = writeln!(
            s,
            "run one with `joue NAME`; `joue NAME --help` says what it takes"
        );
        s
    });
    Exit::Ok
}

fn run_tests(catalog: &Catalog, args: &RunTestArgs, out: &Output) -> Exit {
    let routines: Vec<&Routine> = match &args.routine {
        Some(name) => match catalog.get(name) {
            Some(r) => vec![r],
            None => {
                return out.error_with_hint(
                    Exit::NotFound,
                    &format!("no routine `{name}`"),
                    Some("see `joue --list`"),
                );
            }
        },
        None => catalog
            .routines()
            .iter()
            .filter(|r| r.state != State::Untrusted)
            .collect(),
    };
    let log_dir = jouet_output::paths::log_dir();
    let reports: Vec<testing::Report> = routines
        .iter()
        .map(|r| testing::test(catalog, r, &log_dir))
        .collect();
    let ok = reports.iter().all(|r| r.ok);
    #[derive(Serialize)]
    struct Tested<'a> {
        ok: bool,
        routines: &'a [testing::Report],
    }
    out.result(
        &Tested {
            ok,
            routines: &reports,
        },
        || {
            let mut s = String::new();
            for report in &reports {
                if let Some(error) = &report.error {
                    let _ = writeln!(s, "FAIL {}: {error}", report.routine);
                }
                for example in &report.examples {
                    let mark = if example.ok { "ok  " } else { "FAIL" };
                    let _ = writeln!(s, "{mark} {}: {}", report.routine, example.name);
                    for problem in &example.problems {
                        for (i, line) in problem.lines().enumerate() {
                            let _ =
                                writeln!(s, "{}{line}", if i == 0 { "     - " } else { "       " });
                        }
                    }
                }
            }
            let examples: usize = reports.iter().map(|r| r.examples.len()).sum();
            let failed: usize = reports
                .iter()
                .map(|r| {
                    r.examples.iter().filter(|e| !e.ok).count() + usize::from(r.error.is_some())
                })
                .sum();
            let _ = writeln!(
                s,
                "{} routine{}, {examples} example{}, {}",
                reports.len(),
                if reports.len() == 1 { "" } else { "s" },
                if examples == 1 { "" } else { "s" },
                if failed == 0 {
                    "all passed".to_string()
                } else {
                    format!("{failed} failed")
                }
            );
            s
        },
    );
    if ok { Exit::Ok } else { Exit::Failed }
}

#[derive(Serialize)]
struct Trusted {
    dir: String,
    trusted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    routines: Option<usize>,
}

fn trust(cwd: &Path, trust: bool, out: &Output) -> Exit {
    let Some(dir) = rules::repo_routine_dir(cwd) else {
        return out.error(
            Exit::NotFound,
            "no .jouet/routines directory at the root of this repository",
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
    let count = routine::luau_files(&dir).map(|f| f.len()).ok();
    let report = Trusted {
        dir: dir.display().to_string(),
        trusted: trust,
        routines: if trust { count } else { None },
    };
    out.result(&report, || match (trust, changed) {
        (true, _) => format!(
            "trusted {} ({} routines); a change to any of them needs trusting again\n",
            report.dir,
            count.unwrap_or(0)
        ),
        (false, true) => format!("no longer trusting {}\n", report.dir),
        (false, false) => format!("{} was not trusted\n", report.dir),
    });
    Exit::Ok
}

/// Runs jouet's own commands for a routine, by running this binary with
/// `--json`; their progress goes to stderr.
struct Jouet {
    exe: PathBuf,
}

impl Host for Jouet {
    fn jouet(&mut self, argv: &[String]) -> Result<StepOutput, String> {
        let output = Command::new(&self.exe)
            .arg("--json")
            .args(argv)
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

#[cfg(test)]
mod tests {
    #[test]
    fn every_command_a_routine_can_run_is_in_this_jouet() {
        for command in jouet_routines::prims::COMMANDS {
            assert!(crate::spec::has_command(command.path), "{}", command.name());
        }
    }
}
