//! fnox (<https://fnox.jdx.dev>), which keeps a project's secrets encrypted
//! or in a vault and hands them to programs as environment variables. jouet
//! uses its CLI: it finds a project that uses fnox, reads the secret names
//! its config defines (never their values), runs `fnox scan --values` before
//! `jouet commit`, and notices when the agent runs under `fnox proxy run`.

use jouet_git::Error;
use jouet_git::commit::{ScanStatus, Scanner, SecretFinding, SecretScan};
use jouet_output::exit::Exit;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The config files fnox loads from each directory, besides profile files
/// (`fnox.<profile>.toml`).
const CONFIGS: &[&str] = &[
    "fnox.toml",
    ".fnox.toml",
    ".config/fnox.toml",
    "fnox.local.toml",
    ".fnox.local.toml",
    ".config/fnox.local.toml",
];

/// A project that uses fnox, as its config files describe it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Project {
    /// The config files found, nearest directory last.
    pub configs: Vec<PathBuf>,
    /// Every secret name the configs define, in any profile.
    pub secrets: BTreeSet<String>,
    /// The plaintext files a `plain` provider reads secrets from, made
    /// absolute.
    pub files: Vec<PathBuf>,
}

impl Project {
    /// The project around `dir`: every fnox config in it and the directories
    /// above, as fnox itself loads them (the global config aside). `None`
    /// when there is none.
    pub(crate) fn find(dir: &Path) -> Option<Self> {
        let dir = std::path::absolute(dir).ok()?;
        let mut configs = Vec::new();
        for ancestor in dir.ancestors() {
            let mut here = Vec::new();
            for name in CONFIGS {
                let path = ancestor.join(name);
                if path.is_file() {
                    here.push(path);
                }
            }
            here.extend(profile_files(ancestor));
            configs.splice(0..0, here);
        }
        if configs.is_empty() {
            return None;
        }
        let mut project = Self {
            configs,
            ..Self::default()
        };
        for path in project.configs.clone() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                project.read(&text, path.parent().unwrap_or(Path::new(".")));
            }
        }
        project.files.sort();
        project.files.dedup();
        Some(project)
    }

    /// Adds what one config file defines. `base` is its directory, which
    /// relative `file` paths resolve against.
    fn read(&mut self, text: &str, base: &Path) {
        let Ok(doc) = text.parse::<toml::Table>() else {
            return;
        };
        let mut tables = vec![&doc];
        if let Some(profiles) = doc.get("profiles").and_then(toml::Value::as_table) {
            tables.extend(profiles.values().filter_map(toml::Value::as_table));
        }
        for table in tables {
            if let Some(secrets) = table.get("secrets").and_then(toml::Value::as_table) {
                self.secrets.extend(secrets.keys().cloned());
            }
            let providers = table.get("providers").and_then(toml::Value::as_table);
            for provider in providers.into_iter().flat_map(toml::Table::values) {
                let plain = provider.get("type").and_then(toml::Value::as_str) == Some("plain");
                if let (true, Some(file)) =
                    (plain, provider.get("file").and_then(toml::Value::as_str))
                {
                    self.files.push(resolve(base, file));
                }
            }
        }
    }
}

/// What the hook's rules see of the fnox project around `cwd`, if any.
pub(crate) fn context(cwd: &Path) -> Option<jouet_rules::Fnox> {
    let project = Project::find(cwd)?;
    Some(jouet_rules::Fnox {
        secrets: project.secrets.into_iter().collect(),
        files: project
            .files
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect(),
    })
}

/// `fnox.<profile>.toml` and `.fnox.<profile>.toml` in `dir` and its
/// `.config`, sorted, local overrides aside.
fn profile_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (sub, prefixes) in [("", &["fnox.", ".fnox."][..]), (".config", &["fnox."][..])] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let profile = prefixes
                .iter()
                .find_map(|p| name.strip_prefix(p))
                .and_then(|rest| rest.strip_suffix(".toml"));
            if profile.is_some_and(|p| !p.is_empty() && p != "local" && !p.contains('.'))
                && entry.path().is_file()
            {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

/// A config's `file`, with `~` expanded and made absolute against `base`.
fn resolve(base: &Path, file: &str) -> PathBuf {
    let path = match file.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().map_or_else(|| PathBuf::from(file), |h| h.join(rest)),
        None => PathBuf::from(file),
    };
    normalize(&base.join(path))
}

/// Folds `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether this process runs under `fnox proxy run`, which gives the
/// command placeholder credentials. fnox sets `FNOX_PROXY_MCP=1` in the
/// command's environment so `fnox mcp` relays to the proxy.
pub(crate) fn under_proxy() -> bool {
    std::env::var("FNOX_PROXY_MCP").is_ok_and(|v| v == "1")
}

/// Whether `fnox` is on `PATH`.
pub(crate) fn installed() -> bool {
    crate::guide::on_path("fnox")
}

/// What `jouet commit` tells someone in a fnox project where fnox is not
/// installed.
pub(crate) fn skipped_scan() -> SecretScan {
    SecretScan {
        tool: "fnox".into(),
        status: ScanStatus::Skipped,
        files_scanned: None,
        findings: Vec::new(),
        reason: Some(
            "this repository uses fnox, but fnox is not installed, so the commit was not scanned for secret values"
                .into(),
        ),
        notes: Vec::new(),
    }
}

/// `fnox scan --values --staged` over the index `jouet commit` builds.
pub(crate) struct Scan {
    /// The directory fnox runs in, which decides the config it loads.
    pub dir: PathBuf,
}

impl Scanner for Scan {
    fn scan(&self, _top: &Path, index: &Path) -> Result<SecretScan, Error> {
        let mut command = Command::new("fnox");
        command
            .args(["scan", "--values", "--staged", "--format", "json"])
            .current_dir(&self.dir)
            .env("GIT_INDEX_FILE", index)
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !jouet_output::display::person_at_terminal() {
            command
                .env("FNOX_NON_INTERACTIVE", "1")
                .stdin(Stdio::null());
        }
        let out = command.output().map_err(|e| Error {
            exit: Exit::Precondition,
            message: format!("running fnox scan: {e}"),
            hint: Some(DISABLE.into()),
        })?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        read_scan(
            out.status.code(),
            &String::from_utf8_lossy(&out.stdout),
            &stderr,
            under_proxy(),
        )
    }
}

const DISABLE: &str = "fix what fnox reports, or set [commit] secret_scan = false (JOUET_COMMIT_SECRET_SCAN=0) to commit without the scan";

/// Reads `fnox scan --format json`'s result. fnox exits 1 both when it finds
/// a secret and when it fails, so a finding is told apart by its report on
/// stdout. A scan that failed refuses the commit, unless the agent runs
/// under `fnox proxy run`: it was only given placeholders, and fnox there
/// usually cannot resolve the real values.
fn read_scan(
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
    proxied: bool,
) -> Result<SecretScan, Error> {
    let report: Option<serde_json::Value> = serde_json::from_str(stdout).ok();
    let notes = notes(stderr);
    let findings: Vec<SecretFinding> = report
        .as_ref()
        .and_then(|r| r.get("findings"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|f| SecretFinding {
            secret: f
                .get("secret")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(unnamed)")
                .to_string(),
            path: f
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            line: f
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
        })
        .collect();
    let files_scanned = report
        .as_ref()
        .and_then(|r| r.pointer("/summary/files_scanned"))
        .and_then(serde_json::Value::as_u64);
    let scan = |status, findings| SecretScan {
        tool: "fnox".into(),
        status,
        files_scanned,
        findings,
        reason: None,
        notes: notes.clone(),
    };
    match (code, report.is_some()) {
        (Some(0), true) => Ok(scan(ScanStatus::Clean, Vec::new())),
        (Some(1), true) if !findings.is_empty() => Ok(scan(ScanStatus::Found, findings)),
        _ => {
            let why = failure(stderr, code);
            if proxied {
                return Ok(SecretScan {
                    reason: Some(format!(
                        "fnox scan failed ({why}); the agent runs under fnox proxy run, so it was only given placeholders"
                    )),
                    ..scan(ScanStatus::Skipped, Vec::new())
                });
            }
            Err(Error {
                exit: Exit::Precondition,
                message: format!(
                    "fnox could not scan the commit for secret values, so nothing was committed: {why}"
                ),
                hint: Some(DISABLE.into()),
            })
        }
    }
}

/// fnox's warnings about what it could not check. They name secrets, never
/// their values.
fn notes(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .map(str::trim)
        .filter_map(|l| {
            l.strip_prefix("Warning: ")
                .or_else(|| l.strip_prefix("Note: "))
        })
        .map(str::to_string)
        .collect()
}

/// The gist of a failed scan: the error miette marks with `×` (its wrapped
/// lines joined), else fnox's first line of stderr that is not a warning.
pub(crate) fn failure(stderr: &str, code: Option<i32>) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("Warning: ") && !l.starts_with("Note: "))
        .collect();
    let marked = lines.iter().position(|l| l.starts_with('×')).map(|at| {
        let mut text = lines[at].trim_start_matches('×').trim().to_string();
        for more in lines[at + 1..].iter().map_while(|l| l.strip_prefix('│')) {
            text.push(' ');
            text.push_str(more.trim());
        }
        text
    });
    let line = marked.or_else(|| {
        lines
            .iter()
            .find(|l| !l.is_empty())
            .map(|l| (*l).to_string())
    });
    let code = code.map_or("a signal".to_string(), |c| format!("exit {c}"));
    match line {
        Some(line) => format!("{line} ({code})"),
        None => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_every_config_from_here_up() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("fnox.toml"),
            r#"
[providers.age]
type = "age"
recipients = ["age1x"]

[providers.dotenv]
type = "plain"
file = "secrets.env"
format = "env"

[secrets]
DATABASE_URL = { provider = "age", value = "YWdl" }

[profiles.prod.secrets]
DEPLOY_TOKEN = { provider = "age", value = "YWdl" }
"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("svc/.config")).unwrap();
        std::fs::write(
            root.join("svc/.config/fnox.toml"),
            "[secrets]\nAPI_KEY = { default = \"x\" }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("svc/fnox.staging.toml"),
            "[secrets]\nSTAGING_KEY = { default = \"x\" }\n",
        )
        .unwrap();

        let project = Project::find(&root.join("svc")).unwrap();

        let names: Vec<&str> = project.secrets.iter().map(String::as_str).collect();
        assert_eq!(
            names,
            ["API_KEY", "DATABASE_URL", "DEPLOY_TOKEN", "STAGING_KEY"]
        );
        assert_eq!(project.files, [root.join("secrets.env")]);
        assert_eq!(project.configs[0], root.join("fnox.toml"));
        assert!(Project::find(&tempfile::tempdir().unwrap().path().join("x")).is_none());
    }

    #[test]
    fn a_clean_scan() {
        let scan = read_scan(
            Some(0),
            r#"{"findings":[],"summary":{"files_scanned":2,"files_with_findings":0,"findings":0}}"#,
            "Warning: 1 secret(s) have no value and were not checked: STRIPE_KEY\n",
            false,
        )
        .unwrap();
        assert_eq!(scan.status, ScanStatus::Clean);
        assert_eq!(scan.files_scanned, Some(2));
        assert_eq!(
            scan.notes,
            ["1 secret(s) have no value and were not checked: STRIPE_KEY"]
        );
    }

    #[test]
    fn a_finding_names_the_secret_and_file() {
        let stdout = r#"{
  "findings": [
    {"path": "config/app.env", "line": 3, "column": 14, "detector": "resolved-secret",
     "severity": "high", "redacted": "****", "secret": "DATABASE_URL"}
  ],
  "summary": {"files_scanned": 1, "files_with_findings": 1, "findings": 1}
}"#;
        let scan = read_scan(Some(1), stdout, "", false).unwrap();
        assert_eq!(scan.status, ScanStatus::Found);
        assert_eq!(
            scan.findings,
            [SecretFinding {
                secret: "DATABASE_URL".into(),
                path: "config/app.env".into(),
                line: 3,
            }]
        );
    }

    #[test]
    fn a_failed_scan_refuses_unless_proxied() {
        let stderr = "Warning: 1 secret(s) have no value and were not checked: A\nError: fnox::config::error\n\n  × Configuration error: None of the profile's secrets could be resolved, so\n  │ nothing was scanned: A\n\n";
        let err = read_scan(Some(1), "", stderr, false).unwrap_err();
        assert_eq!(err.exit, Exit::Precondition);
        assert!(
            err.message.contains(
                "Configuration error: None of the profile's secrets could be resolved, so nothing was scanned: A (exit 1)"
            ),
            "{}",
            err.message
        );
        let scan = read_scan(Some(1), "", stderr, true).unwrap();
        assert_eq!(scan.status, ScanStatus::Skipped);
        assert!(scan.reason.unwrap().contains("fnox proxy run"));
    }
}
