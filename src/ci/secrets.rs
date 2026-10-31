//! `jouet ci run --secrets-from fnox`: the `secrets.*` the jobs that will
//! run read, resolved through fnox and handed to jactionlint in a 0600
//! `--secret-file` that is deleted afterwards. Values never reach the
//! command line or jouet's output; jactionlint masks them in its transcript.

use super::Refusal;
use crate::fnox;
use jouet_output::exit::Exit;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// What `--secrets-from` gave the run, by name.
#[derive(Debug, Serialize)]
pub(crate) struct Given {
    /// Where the values came from: `fnox`.
    pub(crate) from: &'static str,
    /// Secrets the jobs read that were passed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) given: Vec<String>,
    /// Secrets the jobs read that fnox does not define or could not
    /// resolve. They are empty in the run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) missing: Vec<String>,
    /// Secrets whose value spans lines, which a `--secret-file` cannot
    /// hold. They are empty in the run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) multiline: Vec<String>,
}

/// The secrets for a run, and the file that holds them while it lasts.
pub(crate) struct Secrets {
    pub(crate) given: Given,
    /// Deleted when dropped. `None` when there was nothing to give.
    pub(crate) file: Option<tempfile::NamedTempFile>,
}

static DOTTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsecrets\.([A-Za-z_][A-Za-z0-9_-]*)").expect("valid regex"));
static INDEXED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bsecrets\[\s*['"]([A-Za-z_][A-Za-z0-9_-]*)['"]\s*\]"#).expect("valid regex")
});
static LOCAL_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*uses:\s*['"]?(\./[^'"\s#]+\.ya?ml)"#).expect("valid regex"));

/// The names a text reads as `secrets.NAME` or `secrets['NAME']`.
fn names_in(text: &str, out: &mut BTreeSet<String>) {
    for re in [&*DOTTED, &*INDEXED] {
        out.extend(re.captures_iter(text).map(|c| c[1].to_string()));
    }
}

/// A workflow's text outside its jobs (`on:`, `env:`, `defaults:`), and
/// each job's block by ID, read by indentation rather than parsed: the
/// keys under the top-level `jobs:`.
fn split_jobs(text: &str) -> (String, Vec<(String, String)>) {
    let mut outside = String::new();
    let mut jobs: Vec<(String, String)> = Vec::new();
    let mut in_jobs = false;
    let mut indent: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let blank = trimmed.is_empty() || trimmed.starts_with('#');
        let depth = line.len() - trimmed.len();
        if !blank && depth == 0 {
            in_jobs = trimmed.trim_end() == "jobs:";
            indent = None;
            if in_jobs {
                continue;
            }
        }
        if !in_jobs {
            outside.push_str(line);
            outside.push('\n');
            continue;
        }
        if !blank && indent.is_none_or(|i| depth <= i) {
            indent = Some(depth);
            let id = trimmed
                .split(':')
                .next()
                .unwrap_or_default()
                .trim_matches(['"', '\''])
                .to_string();
            jobs.push((id, String::new()));
        }
        if let Some((_, block)) = jobs.last_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    (outside, jobs)
}

/// The secrets the selected jobs read: in their own blocks, in what the
/// workflow sets for every job, and in a local reusable workflow one of
/// them calls. `selected` maps a workflow file to its selected job IDs.
pub(crate) fn referenced(cwd: &Path, selected: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (workflow, ids) in selected {
        let Ok(text) = std::fs::read_to_string(cwd.join(workflow)) else {
            continue;
        };
        let (outside, jobs) = split_jobs(&text);
        names_in(&outside, &mut names);
        for (_, block) in jobs.iter().filter(|(id, _)| ids.contains(id)) {
            names_in(block, &mut names);
            for line in block.lines() {
                if let Some(called) = LOCAL_CALL.captures(line)
                    && let Ok(text) = std::fs::read_to_string(cwd.join(&called[1]))
                {
                    names_in(&text, &mut names);
                }
            }
        }
    }
    names
}

/// Resolves `names` through fnox and writes the ones it has to a 0600
/// temporary file of `NAME="value"` lines, as jactionlint's
/// `--secret-file` reads them.
pub(crate) fn from_fnox(cwd: &Path, names: &BTreeSet<String>) -> Result<Secrets, Refusal> {
    let Some(project) = fnox::Project::find(cwd) else {
        return Err(Refusal::new(
            Exit::Precondition,
            "--secrets-from fnox needs a fnox project here, and there is no fnox.toml in this directory or above",
        ));
    };
    if !fnox::installed() {
        return Err(Refusal::new(
            Exit::Precondition,
            "--secrets-from fnox needs fnox, and it is not installed",
        )
        .hint("mise use -g fnox"));
    }
    let (known, mut missing): (Vec<String>, Vec<String>) = names
        .iter()
        .cloned()
        .partition(|n| project.secrets.contains(n));
    let values = resolve(cwd, &known)?;
    let mut given = Given {
        from: "fnox",
        given: Vec::new(),
        missing: Vec::new(),
        multiline: Vec::new(),
    };
    let mut text = String::new();
    for name in &known {
        match values.get(name) {
            Some(value) if value.contains(['\n', '\r']) => given.multiline.push(name.clone()),
            // jactionlint trims a value and drops one pair of quotes around
            // it, so the quotes keep its spaces and its own quotes.
            Some(value) => {
                text.push_str(&format!("{name}=\"{value}\"\n"));
                given.given.push(name.clone());
            }
            None => missing.push(name.clone()),
        }
    }
    missing.sort();
    given.missing = missing;
    if given.given.is_empty() {
        return Ok(Secrets { given, file: None });
    }
    let io = |e: std::io::Error| {
        Refusal::new(
            Exit::Failed,
            format!("writing the secrets for jactionlint: {e}"),
        )
    };
    // `NamedTempFile` creates the file readable by its owner alone.
    let mut file = tempfile::Builder::new()
        .prefix("jouet-ci-secrets-")
        .tempfile()
        .map_err(io)?;
    restrict(file.path()).map_err(io)?;
    file.write_all(text.as_bytes()).map_err(io)?;
    file.flush().map_err(io)?;
    Ok(Secrets {
        given,
        file: Some(file),
    })
}

/// Makes sure only the owner can read the file, whatever the umask.
fn restrict(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The values fnox has for `names`, from `fnox export --format json`. All
/// at once first; when that fails, as it does for a name the active profile
/// does not define, one at a time, so one bad name does not lose the rest.
fn resolve(cwd: &Path, names: &[String]) -> Result<BTreeMap<String, String>, Refusal> {
    if names.is_empty() {
        return Ok(BTreeMap::new());
    }
    if let Ok(values) = export(cwd, names) {
        return Ok(values);
    }
    let mut values = BTreeMap::new();
    let mut first_error = None;
    for name in names {
        match export(cwd, std::slice::from_ref(name)) {
            Ok(found) => values.extend(found),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    match first_error {
        Some(e) if values.is_empty() => Err(Refusal::new(
            Exit::Precondition,
            format!("fnox could not resolve the secrets the jobs read: {e}"),
        )),
        _ => Ok(values),
    }
}

/// One `fnox export --format json NAME...`. Values it cannot resolve are
/// left out rather than failing the export.
fn export(cwd: &Path, names: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut command = Command::new("fnox");
    command
        .args(["export", "--format", "json"])
        .args(names)
        .current_dir(cwd)
        .env("FNOX_IF_MISSING", "warn")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !jouet_output::display::person_at_terminal() {
        command
            .env("FNOX_NON_INTERACTIVE", "1")
            .stdin(Stdio::null());
    }
    let out = command
        .output()
        .map_err(|e| format!("running fnox export: {e}"))?;
    if !out.status.success() {
        return Err(fnox::failure(
            &String::from_utf8_lossy(&out.stderr),
            out.status.code(),
        ));
    }
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("reading fnox export's JSON: {e}"))?;
    Ok(doc
        .get("secrets")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW: &str = r#"name: CI
on: push
env:
  REGISTRY_TOKEN: ${{ secrets.REGISTRY_TOKEN }}
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: make
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
  "deploy":
    needs: build
    steps:
      - run: ./deploy
        env:
          KEY: ${{ secrets['DEPLOY_KEY'] }}
  release:
    uses: ./.github/workflows/release.yml
    secrets: inherit
"#;

    fn selected(ids: &[&str]) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([(
            ".github/workflows/ci.yml".to_string(),
            ids.iter().map(|s| (*s).to_string()).collect(),
        )])
    }

    #[test]
    fn only_the_selected_jobs_and_what_they_inherit_count() {
        let dir = tempfile::tempdir().unwrap();
        let workflows = dir.path().join(".github/workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("ci.yml"), WORKFLOW).unwrap();
        std::fs::write(
            workflows.join("release.yml"),
            "on: workflow_call\njobs:\n  publish:\n    steps:\n      - run: echo ${{ secrets.CRATES_TOKEN }}\n",
        )
        .unwrap();

        let names = |ids: &[&str]| -> Vec<String> {
            referenced(dir.path(), &selected(ids)).into_iter().collect()
        };
        assert_eq!(names(&["build"]), ["NPM_TOKEN", "REGISTRY_TOKEN"]);
        assert_eq!(
            names(&["build", "deploy"]),
            ["DEPLOY_KEY", "NPM_TOKEN", "REGISTRY_TOKEN"]
        );
        assert_eq!(names(&["release"]), ["CRATES_TOKEN", "REGISTRY_TOKEN"]);
    }

    #[test]
    fn jobs_are_split_by_their_indentation() {
        let (outside, jobs) = split_jobs(WORKFLOW);
        assert!(outside.contains("REGISTRY_TOKEN"));
        assert!(!outside.contains("NPM_TOKEN"));
        let ids: Vec<&str> = jobs.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["build", "deploy", "release"]);
        assert!(jobs[1].1.contains("DEPLOY_KEY"));
    }
}
