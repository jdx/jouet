//! Finding a workflow and job in the plan `jactionlint run --list --json`
//! prints, from the names GitHub shows: a workflow's `name:` and a check's
//! name.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Plan {
    /// The path of the workflow file, relative to the repository.
    pub(crate) workflow: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) jobs: Vec<PlanJob>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PlanJob {
    pub(crate) id: String,
    /// Whether the job runs, given the jobs asked for and what they need.
    #[serde(default)]
    pub(crate) selected: bool,
    #[serde(default)]
    pub(crate) cells: Vec<Cell>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Cell {
    /// The job ID and the matrix values, such as `test (ubuntu-latest, 1)`.
    pub(crate) name: String,
    /// The evaluated `name:` of the job, when jactionlint reports it.
    #[serde(default)]
    pub(crate) display: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    Found {
        workflow: String,
        job: String,
    },
    /// No workflow has such a job; the names it could have been.
    Missing(Vec<String>),
    /// Several workflows have one; their files.
    Ambiguous(Vec<String>),
}

pub(crate) fn parse(json: &str) -> Result<Vec<Plan>, String> {
    serde_json::from_str(json).map_err(|e| format!("reading the plan of jactionlint run: {e}"))
}

/// The file of the workflow `wanted` names: a path, a file name with or
/// without its extension, or the workflow's `name:`.
pub(crate) fn workflow<'a>(plans: &'a [Plan], wanted: &str) -> Option<&'a Plan> {
    let by = |f: &dyn Fn(&Plan) -> bool| plans.iter().find(|p| f(p));
    by(&|p| p.workflow == wanted || file_name(&p.workflow) == wanted)
        .or_else(|| by(&|p| stem(&p.workflow) == wanted))
        .or_else(|| by(&|p| p.name.as_deref() == Some(wanted)))
        .or_else(|| {
            by(&|p| {
                p.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
            })
        })
}

/// The job behind a check name, in `plans`.
pub(crate) fn check(plans: &[&Plan], name: &str) -> Lookup {
    // A job of a reusable workflow shows as `caller / job`; jactionlint
    // runs the caller.
    let caller = name.split_once(" / ").map(|(c, _)| c);
    let mut found: Vec<(&str, &str)> = Vec::new();
    for plan in plans {
        for job in &plan.jobs {
            let hit = |n: &str| {
                job.id == n
                    || job
                        .cells
                        .iter()
                        .any(|c| c.name == n || c.display.as_deref() == Some(n))
            };
            if hit(name) || caller.is_some_and(hit) {
                found.push((&plan.workflow, &job.id));
                break;
            }
        }
    }
    match found.as_slice() {
        [(workflow, job)] => Lookup::Found {
            workflow: (*workflow).to_string(),
            job: (*job).to_string(),
        },
        [] => {
            let mut names: Vec<String> = plans
                .iter()
                .flat_map(|p| &p.jobs)
                .flat_map(|j| {
                    j.cells
                        .iter()
                        .map(|c| c.display.clone().unwrap_or_else(|| c.name.clone()))
                })
                .collect();
            names.dedup();
            Lookup::Missing(names)
        }
        many => Lookup::Ambiguous(many.iter().map(|(w, _)| (*w).to_string()).collect()),
    }
}

/// Whether the plan names any job by its `name:`. A jactionlint whose JSON
/// plan has no display names still shows them in the text one.
pub(crate) fn has_display_names(plans: &[Plan]) -> bool {
    plans
        .iter()
        .flat_map(|p| &p.jobs)
        .flat_map(|j| &j.cells)
        .any(|c| c.display.is_some())
}

/// Fills in the display names from the text of `jactionlint run --list`:
/// a workflow's path at the start of a line, then one line per cell that
/// starts with its name and, when the job has a `name:`, holds
/// `   name: DISPLAY`.
pub(crate) fn add_display_names(plans: &mut [Plan], text: &str) {
    let mut current: Option<usize> = None;
    for line in text.lines() {
        if !line.starts_with(' ') {
            let path = line.split("  (").next().unwrap_or(line).trim();
            current = plans.iter().position(|p| p.workflow == path);
            continue;
        }
        let (Some(i), Some(body)) = (current, line.strip_prefix("    ")) else {
            continue;
        };
        let Some((_, rest)) = body.split_once("   name: ") else {
            continue;
        };
        let display = rest.split("   ").next().unwrap_or(rest);
        // The longest cell name the line starts with, since a cell whose
        // matrix values cannot be shown gets them appended.
        let cell = plans[i]
            .jobs
            .iter_mut()
            .flat_map(|j| &mut j.cells)
            .filter(|c| {
                body.strip_prefix(c.name.as_str())
                    .is_some_and(|after| after.is_empty() || after.starts_with(' '))
            })
            .max_by_key(|c| c.name.len());
        if let Some(cell) = cell {
            cell.display = Some(display.to_string());
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.strip_suffix(".yml")
        .or_else(|| name.strip_suffix(".yaml"))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"[
      {"workflow": ".github/workflows/ci.yml", "name": "CI", "event": "push", "jobs": [
        {"id": "test", "stage": 1, "selected": true, "cells": [
          {"name": "test (ubuntu-latest, 1.23)", "matrix": {"os": "ubuntu-latest", "go": "1.23"}},
          {"name": "test (macos-latest, 1.23)"}]},
        {"id": "lint", "stage": 1, "selected": true, "cells": [{"name": "lint", "display": "Lint"}]},
        {"id": "release", "stage": 2, "selected": true, "uses": "./.github/workflows/release.yml", "cells": [{"name": "release"}]}
      ]},
      {"workflow": ".github/workflows/docs.yaml", "name": "Docs", "event": "push", "jobs": [
        {"id": "lint", "stage": 1, "selected": true, "cells": [{"name": "lint"}]}
      ]}
    ]"#;

    #[test]
    fn workflows_by_path_file_stem_and_name() {
        let plans = parse(PLAN).unwrap();
        for wanted in [".github/workflows/ci.yml", "ci.yml", "ci", "CI", "cI"] {
            assert_eq!(
                workflow(&plans, wanted).unwrap().workflow,
                ".github/workflows/ci.yml"
            );
        }
        assert_eq!(
            workflow(&plans, "docs").unwrap().workflow,
            ".github/workflows/docs.yaml"
        );
        assert_eq!(
            workflow(&plans, "docs").map(|p| p.name.as_deref()),
            Some(Some("Docs"))
        );
        assert!(workflow(&plans, "deploy").is_none());
    }

    #[test]
    fn checks_by_cell_display_name_and_caller() {
        let plans = parse(PLAN).unwrap();
        let all: Vec<&Plan> = plans.iter().collect();
        let found = |workflow: &str, job: &str| Lookup::Found {
            workflow: workflow.into(),
            job: job.into(),
        };
        assert_eq!(
            check(&all, "test (macos-latest, 1.23)"),
            found(".github/workflows/ci.yml", "test")
        );
        assert_eq!(
            check(&all, "Lint"),
            found(".github/workflows/ci.yml", "lint")
        );
        assert_eq!(
            check(&all, "release / publish"),
            found(".github/workflows/ci.yml", "release")
        );
        assert_eq!(
            check(&all, "lint"),
            Lookup::Ambiguous(vec![
                ".github/workflows/ci.yml".into(),
                ".github/workflows/docs.yaml".into()
            ])
        );
        let docs = [workflow(&plans, "Docs").unwrap()];
        assert_eq!(
            check(&docs, "lint"),
            found(".github/workflows/docs.yaml", "lint")
        );
        assert!(has_display_names(&plans));
        let Lookup::Missing(names) = check(&all, "unit") else {
            panic!("unit is not a job");
        };
        assert!(names.contains(&"Lint".to_string()), "{names:?}");
        assert!(
            names.contains(&"test (ubuntu-latest, 1.23)".to_string()),
            "{names:?}"
        );
    }

    #[test]
    fn display_names_from_the_text_plan() {
        let mut plans = parse(
            r#"[{"workflow": ".github/workflows/ci.yml", "name": "CI", "event": "push", "jobs": [
              {"id": "build", "stage": 1, "selected": true, "cells": [{"name": "build"}]},
              {"id": "test", "stage": 2, "selected": true, "cells": [
                {"name": "test (ubuntu-latest)"}, {"name": "test (macos-latest)"}]}]}]"#,
        )
        .unwrap();
        assert!(!has_display_names(&plans));
        let text = "\
.github/workflows/ci.yml  (CI)
  stage 1
    build   runs-on: ubuntu-latest
  stage 2
    test (ubuntu-latest)   needs: build   runs-on: ubuntu-latest   name: Unit tests (ubuntu-latest)
    test (macos-latest)   needs: build   runs-on: macos-latest   => skipped: needs macOS   name: Unit tests (macos-latest)   => if is true
";
        add_display_names(&mut plans, text);
        let all: Vec<&Plan> = plans.iter().collect();
        for name in ["Unit tests (ubuntu-latest)", "Unit tests (macos-latest)"] {
            assert_eq!(
                check(&all, name),
                Lookup::Found {
                    workflow: ".github/workflows/ci.yml".into(),
                    job: "test".into()
                },
                "{name}"
            );
        }
        assert_eq!(plans[0].jobs[0].cells[0].display, None);
    }
}
