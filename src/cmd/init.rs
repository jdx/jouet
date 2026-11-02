//! Inspect a repository and preview small, evidence-backed project settings.

use super::Output;
use crate::cli::InitArgs;
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

const RULE_PATH: &str = ".jouet/rules/project-test.toml";
const PYTEST_PATH: &str = ".jouet/test.toml";
const PYTEST_CONFIG: &str = "[pytest]\naffected_args = [\"-m\", \"not download\"]\n";
const RULE: &str = r#"#:schema https://jouet.jdx.dev/schema/rules.json
[[rule]]
name = "project-test-task"
why = "The project defines a mise test task, which may cover more than cargo test."
mode = "warn"
message = "This project defines `mise run test`. Check its task and the project instructions when choosing a full test run; a focused cargo test may still be appropriate during iteration."

[rule.match]
command = "cargo test"
max_args = 0

[rule.examples]
fires = ["cargo test"]
quiet = ["cargo test specific_test", "mise run test"]
"#;

#[derive(Serialize)]
struct Proposal {
    path: &'static str,
    evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    caution: Option<&'static str>,
    content: &'static str,
    exists: bool,
    written: bool,
}

/// Only a literal pytest marker in this target is evidence: do not evaluate
/// Makefile variables, includes or shell commands to infer what a task does.
fn offline_pytest(makefile: &str) -> bool {
    let mut in_target = false;
    for line in makefile.lines() {
        if !in_target {
            in_target = line.strip_prefix("offlinetest:").is_some();
        } else if line.starts_with('\t') {
            let command = line.trim_start();
            let pytest = [
                "pytest ",
                "python -m pytest ",
                "python3 -m pytest ",
                "$(PYTHON) -m pytest ",
            ]
            .iter()
            .any(|prefix| command.starts_with(prefix));
            if pytest
                && (line.contains("-m \"not download\"") || line.contains("-m 'not download'"))
            {
                return true;
            }
        } else if !line.trim().is_empty() && !line.starts_with('#') {
            return false;
        }
    }
    false
}

#[derive(Serialize)]
struct Report {
    root: String,
    files: Vec<&'static str>,
    tasks: Vec<Task>,
    proposals: Vec<Proposal>,
}

#[derive(Serialize)]
struct Task {
    command: String,
    evidence: String,
}

fn inspect(root: &Path) -> Report {
    let files = [
        "Cargo.toml",
        "pyproject.toml",
        "pytest.ini",
        "package.json",
        "go.mod",
        "mise.toml",
        ".mise.toml",
        "Makefile",
        "justfile",
        "runtests.sh",
        "AGENTS.md",
        ".github/workflows",
    ]
    .into_iter()
    .filter(|name| root.join(name).exists())
    .collect();
    let mut tasks = Vec::new();
    let mut mise = None;
    for name in ["mise.toml", ".mise.toml"] {
        let table = std::fs::read_to_string(root.join(name))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok());
        let Some(declared) = table
            .as_ref()
            .and_then(|t| t.get("tasks"))
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        for task in ["test", "build", "lint", "check"] {
            if declared.get(task).and_then(toml::Value::as_table).is_some() {
                tasks.push(Task {
                    command: format!("mise run {task}"),
                    evidence: format!("{name} [tasks.{task}]"),
                });
                if task == "test" && mise.is_none() {
                    mise = Some(name);
                }
            }
        }
    }
    if let Ok(makefile) = std::fs::read_to_string(root.join("Makefile")) {
        for task in ["test", "offlinetest", "build", "lint", "check"] {
            if makefile.lines().any(|line| {
                line.strip_prefix(task)
                    .is_some_and(|rest| rest.starts_with(':') && !rest.starts_with(":="))
            }) {
                tasks.push(Task {
                    command: format!("make {task}"),
                    evidence: format!("Makefile {task} target"),
                });
            }
        }
    }
    if root.join("runtests.sh").is_file() {
        tasks.push(Task {
            command: "sh runtests.sh".into(),
            evidence: "root runtests.sh".into(),
        });
    }
    if root.join("Cargo.toml").is_file() {
        for verb in ["test", "build"] {
            tasks.push(Task {
                command: format!("cargo {verb} --workspace"),
                evidence: "Cargo.toml (Cargo convention; check project instructions)".into(),
            });
        }
    }
    let mut proposals = Vec::new();
    if root.join("Cargo.toml").is_file()
        && let Some(mise) = mise
    {
        proposals.push(Proposal {
            path: RULE_PATH,
            evidence: format!("Cargo.toml and {mise} [tasks.test]"),
            caution: None,
            content: RULE,
            exists: root.join(RULE_PATH).exists(),
            written: false,
        });
    }
    if root.join("pyproject.toml").is_file()
        && std::fs::read_to_string(root.join("Makefile")).is_ok_and(|text| offline_pytest(&text))
    {
        proposals.push(Proposal {
            path: PYTEST_PATH,
            evidence: "Makefile offlinetest runs pytest with -m 'not download'".into(),
            caution: Some("Selected pytest runs exclude download-marked tests. This does not prove the remaining tests avoid network access; inspect the task and run the full suite when needed."),
            content: PYTEST_CONFIG,
            exists: root.join(PYTEST_PATH).exists(),
            written: false,
        });
    }
    Report {
        root: root.display().to_string(),
        files,
        tasks,
        proposals,
    }
}

pub(crate) fn run(args: &InitArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let Some(root) = cwd.ancestors().find(|dir| dir.join(".git").exists()) else {
        return out.error(
            Exit::Precondition,
            "no Git repository found above the current directory",
        );
    };
    let mut report = inspect(root);
    if args.yes {
        for proposal in &mut report.proposals {
            if proposal.exists {
                continue;
            }
            let mut parents = vec![root.join(".jouet")];
            if proposal.path == RULE_PATH {
                parents.push(root.join(".jouet/rules"));
            }
            for path in parents {
                if path.is_symlink() {
                    return out.error(
                        Exit::Precondition,
                        &format!("refusing to write through symlink {}", path.display()),
                    );
                }
            }
            let path = root.join(proposal.path);
            let result = std::fs::create_dir_all(path.parent().expect("rule has a parent"))
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                })
                .and_then(|mut file| file.write_all(proposal.content.as_bytes()));
            if let Err(e) = result {
                return out.error(Exit::Failed, &format!("writing {}: {e}", path.display()));
            }
            proposal.written = true;
        }
    }
    out.result(&report, || {
        let mut text = format!("Project: {}\nFiles: {}\n", report.root, report.files.join(", "));
        if !report.tasks.is_empty() {
            text.push_str("\nCommands to investigate (inspect their definitions and coverage before running):\n");
            for task in &report.tasks {
                let _ = writeln!(text, "  {}  ({})", task.command, task.evidence);
            }
        }
        if report.proposals.is_empty() {
            text.push_str("No automatic rule proposed for the files found.\n");
        }
        for p in &report.proposals {
            let status = if p.written { "written" } else if p.exists { "already exists; left unchanged" } else { "preview" };
            let _ = writeln!(text, "\n{} ({status}; evidence: {})\n{}", p.path, p.evidence, p.content);
            if let Some(caution) = p.caution {
                let _ = writeln!(text, "{caution}");
            }
        }
        if report.proposals.iter().any(|p| !p.exists && !p.written) {
            text.push_str("Run `jouet init --yes` to write the proposed files.\n");
        }
        if report.proposals.iter().any(|p| p.written) {
            text.push_str("Review the files and test any rules before deciding whether to run `jouet trust`. This command did not trust them.\n");
        }
        text
    });
    Exit::Ok
}
