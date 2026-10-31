//! `joue test`: a routine's examples, run against recorded responses.
//!
//! A routine's table may carry `examples`, a list of runs and what each must
//! come to:
//!
//! ```luau
//! examples = {
//!   {
//!     name = "a conflicted PR is the agent's to act on",
//!     args = { "7", "--repo", "jdx/demo" },
//!     fixture = "fixtures/pr-next/conflict.json",
//!     exit = 10,
//!     output = "act  conflict",
//!     json = { prs = { { action = "conflict" } } },
//!   },
//! }
//! ```
//!
//! - `args` are the words after the routine's name; `yes = true` adds
//!   `--yes`.
//! - `fixture` is a file next to the routine (embedded for a built-in), or
//!   the same table inline. Its `github` list is served by a local fake
//!   GitHub: each entry answers the first request with its `method`, `path`
//!   and (when given) a body containing `match`, with `status`, `body` and
//!   `headers`. Its `jouet` list answers jouet's commands in order, each by
//!   the start of its command line (`"wait ci"`), with `exit_code` and
//!   `json`. Its `git`, `mise` and `ledger` answer those primitives.
//!   Nothing reaches the real GitHub, git or mise, and file edits are only
//!   ever previewed.
//! - `exit` is the exit code (default 0). `output` is text the output must
//!   contain (a string or a list), with an error shown as `jouet: ...`.
//!   `json` must match the `--json` document, run separately: every field
//!   given must be there and equal, and a list must have as many entries.
//!   `planned` lists writes that must have been previewed (`{}` for none),
//!   and `requests` requests that must have been made, each as text found
//!   in `METHOD PATH BODY` or `jouet COMMAND`.

use crate::routine::{Catalog, Routine};
use crate::runtime::{self, Options, Outcome};
use crate::world::{Github, Host, Recorded, StepOutput, World};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The built-in routines' fixtures, by the path their examples name.
const FIXTURES: &[(&str, &str)] = &[
    (
        "fixtures/pr-next.json",
        include_str!("../routines/fixtures/pr-next.json"),
    ),
    (
        "fixtures/land.json",
        include_str!("../routines/fixtures/land.json"),
    ),
    (
        "fixtures/land-pending.json",
        include_str!("../routines/fixtures/land-pending.json"),
    ),
    (
        "fixtures/sweep.json",
        include_str!("../routines/fixtures/sweep.json"),
    ),
    (
        "fixtures/ship-check.json",
        include_str!("../routines/fixtures/ship-check.json"),
    ),
];

#[derive(Debug, Serialize)]
pub struct Report {
    pub routine: String,
    pub ok: bool,
    pub examples: Vec<ExampleReport>,
    /// Why the examples could not be read at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExampleReport {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

/// Runs every example of `routine`. A routine without examples fails.
pub fn test(catalog: &Catalog, routine: &Routine, log_dir: &std::path::Path) -> Report {
    let mut report = Report {
        routine: routine.name.clone(),
        ok: false,
        examples: Vec::new(),
        error: None,
    };
    let examples = match runtime::examples(catalog, routine) {
        Ok(examples) if examples.is_empty() => {
            report.error = Some("it has no examples".into());
            return report;
        }
        Ok(examples) => examples,
        Err(e) => {
            report.error = Some(e.message);
            return report;
        }
    };
    for (i, example) in examples.iter().enumerate() {
        let name = example["name"]
            .as_str()
            .map_or_else(|| format!("example {}", i + 1), str::to_string);
        let problems = run_example(catalog, routine, example, log_dir);
        report.examples.push(ExampleReport {
            name,
            ok: problems.is_empty(),
            problems,
        });
    }
    report.ok = report.examples.iter().all(|e| e.ok);
    report
}

fn run_example(
    catalog: &Catalog,
    routine: &Routine,
    example: &Value,
    log_dir: &std::path::Path,
) -> Vec<String> {
    let mut problems = Vec::new();
    let fixture = match fixture(routine, &example["fixture"]) {
        Ok(fixture) => fixture,
        Err(e) => return vec![e],
    };
    let mut words: Vec<String> = match &example["args"] {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .map(|w| match w {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        _ => return vec!["`args` must be a list of words".into()],
    };
    if example["yes"] == true {
        words.push("--yes".into());
    }
    let text_run = run_once(catalog, routine, &words, &fixture, false, log_dir);
    let (outcome, requests) = match text_run {
        Ok(ran) => ran,
        Err(e) => return vec![e],
    };
    let expected_exit = example["exit"].as_i64().unwrap_or(0);
    if i64::from(outcome.exit_code) != expected_exit {
        let why = outcome
            .error
            .as_ref()
            .map(|f| format!(": {}", f.message))
            .unwrap_or_default();
        problems.push(format!(
            "exited {}, expected {expected_exit}{why}",
            outcome.exit_code
        ));
    }
    let mut shown = outcome.text.clone();
    if let Some(fail) = &outcome.error {
        shown.push_str(&format!("jouet: {}\n", fail.message));
        if let Some(hint) = &fail.hint {
            shown.push_str(&format!("hint: {hint}\n"));
        }
    }
    for want in texts(&example["output"]) {
        if !shown.contains(&want) {
            problems.push(format!(
                "the output lacks `{want}`; it was:\n{}",
                indent(&shown)
            ));
        }
    }
    match &example["planned"] {
        Value::Null => {}
        Value::Array(items) if items.is_empty() && !outcome.planned.is_empty() => {
            problems.push(format!(
                "previewed writes, expected none: {}",
                outcome.planned.join("; ")
            ));
        }
        Value::Object(fields) if fields.is_empty() && !outcome.planned.is_empty() => {
            problems.push(format!(
                "previewed writes, expected none: {}",
                outcome.planned.join("; ")
            ));
        }
        planned => {
            for want in texts(planned) {
                if !outcome.planned.iter().any(|p| p.contains(&want)) {
                    problems.push(format!(
                        "no previewed write matches `{want}`; they were: {}",
                        if outcome.planned.is_empty() {
                            "none".into()
                        } else {
                            outcome.planned.join("; ")
                        }
                    ));
                }
            }
        }
    }
    for want in texts(&example["requests"]) {
        if !requests.iter().any(|r| r.contains(&want)) {
            problems.push(format!(
                "no request matches `{want}`; they were:\n{}",
                indent(&requests.join("\n"))
            ));
        }
    }
    if !example["json"].is_null() {
        match run_once(catalog, routine, &words, &fixture, true, log_dir) {
            Ok((outcome, _)) => {
                let document = document(&outcome);
                let mut path = String::from("$");
                if let Err(e) = contains(&document, &example["json"], &mut path) {
                    problems.push(format!("the JSON {e}; it was {document}"));
                }
            }
            Err(e) => problems.push(e),
        }
    }
    problems
}

/// What `joue --json` prints for an outcome.
pub fn document(outcome: &Outcome) -> Value {
    if let Some(fail) = &outcome.error {
        let mut error = json!({ "code": fail.exit.name(), "message": fail.message });
        if let Some(hint) = &fail.hint {
            error["hint"] = hint.as_str().into();
        }
        return json!({ "error": error });
    }
    let mut value = outcome
        .value
        .clone()
        .unwrap_or_else(|| json!({ "output": outcome.text }));
    if !outcome.planned.is_empty()
        && let Value::Object(fields) = &mut value
    {
        fields.insert("planned".into(), outcome.planned.clone().into());
    }
    value
}

fn run_once(
    catalog: &Catalog,
    routine: &Routine,
    words: &[String],
    fixture: &Value,
    as_json: bool,
    log_dir: &std::path::Path,
) -> Result<(Outcome, Vec<String>), String> {
    let mut args = match routine.parse_args(words) {
        Ok(args) => args,
        Err(crate::ArgError::Help(_)) => return Err("its args asked for --help".into()),
        Err(crate::ArgError::Usage(e)) => return Err(format!("its args do not parse: {e}")),
    };
    args.json = as_json;
    let server = FakeGithub::start(fixture["github"].as_array().cloned().unwrap_or_default())?;
    let requests = server.requests.clone();
    let host = Recording {
        answers: fixture["jouet"].as_array().cloned().unwrap_or_default(),
        requests: requests.clone(),
    };
    let world = World {
        root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        host: Box::new(host),
        github: Github::Fake {
            url: server.url.clone(),
            token: "joue-test".into(),
        },
        ledger: None,
        repo_key: None,
        log_dir: log_dir.to_path_buf(),
        recorded: Some(Recorded::from_fixture(fixture)),
    };
    let options = Options {
        json: as_json,
        yes: args.yes,
        dry_run: args.dry_run,
        testing: true,
    };
    let outcome = runtime::run(catalog, routine, args, world, options);
    let seen = requests.lock().map(|r| r.clone()).unwrap_or_default();
    drop(server);
    Ok((outcome, seen))
}

/// The fixture an example names: a file beside the routine (embedded for a
/// built-in), or a table given inline.
fn fixture(routine: &Routine, named: &Value) -> Result<Value, String> {
    let path = match named {
        Value::Null => return Ok(json!({})),
        Value::Object(_) => return Ok(named.clone()),
        Value::String(path) => path,
        _ => return Err("`fixture` must be a path or a table".into()),
    };
    let text = match routine.origin.dir() {
        None => FIXTURES
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| (*text).to_string())
            .ok_or_else(|| format!("no built-in fixture `{path}`"))?,
        Some(dir) => {
            if std::path::Path::new(path)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(format!(
                    "the fixture `{path}` must be inside {}",
                    dir.display()
                ));
            }
            std::fs::read_to_string(dir.join(path))
                .map_err(|e| format!("reading {}: {e}", dir.join(path).display()))?
        }
    };
    serde_json::from_str(&text).map_err(|e| format!("the fixture `{path}` is not JSON: {e}"))
}

/// A string, or each string in a list.
fn texts(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `actual` has every field `expected` gives, equal; lists must be
/// as long, and match entry by entry.
fn contains(actual: &Value, expected: &Value, path: &mut String) -> Result<(), String> {
    match (actual, expected) {
        (Value::Object(have), Value::Object(want)) => {
            for (key, want) in want {
                let len = path.len();
                path.push('.');
                path.push_str(key);
                match have.get(key) {
                    Some(have) => contains(have, want, path)?,
                    None => return Err(format!("lacks {path}")),
                }
                path.truncate(len);
            }
            Ok(())
        }
        // An empty table in Luau is an empty object, so `{}` stands for an
        // empty list too.
        (Value::Array(have), Value::Object(want)) if want.is_empty() => {
            if have.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "has {} entries at {path}, expected none",
                    have.len()
                ))
            }
        }
        (Value::Array(have), Value::Array(want)) => {
            if have.len() != want.len() {
                return Err(format!(
                    "has {} entries at {path}, expected {}",
                    have.len(),
                    want.len()
                ));
            }
            for (i, (have, want)) in have.iter().zip(want).enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                contains(have, want, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        (have, want) if have == want => Ok(()),
        (have, want) => Err(format!("has {have} at {path}, expected {want}")),
    }
}

/// jouet's commands, answered from a fixture's `jouet` list in order.
struct Recording {
    answers: Vec<Value>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Host for Recording {
    fn jouet(&mut self, argv: &[String]) -> Result<StepOutput, String> {
        let line = argv.join(" ");
        if let Ok(mut seen) = self.requests.lock() {
            seen.push(format!("jouet {line}"));
        }
        let found = self
            .answers
            .iter()
            .position(|a| a["command"].as_str().is_some_and(|c| line.starts_with(c)))
            .ok_or_else(|| format!("the fixture records no answer for `jouet {line}`"))?;
        let answer = self.answers.remove(found);
        Ok(StepOutput {
            exit_code: answer["exit_code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(0),
            json: Some(answer["json"].clone()).filter(|j| !j.is_null()),
        })
    }
}

/// A local GitHub answering from a fixture's `github` list, one request per
/// connection.
pub(crate) struct FakeGithub {
    pub(crate) url: String,
    pub(crate) requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl FakeGithub {
    pub(crate) fn start(responses: Vec<Value>) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("starting the fake GitHub: {e}"))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopping) = (requests.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut used = vec![false; responses.len()];
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let _ = serve(stream, &responses, &mut used, &seen);
            }
        });
        Ok(Self {
            url: format!("http://{addr}"),
            requests,
            stop,
        })
    }
}

impl Drop for FakeGithub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag.
        let _ = std::net::TcpStream::connect(self.url.trim_start_matches("http://"));
    }
}

fn serve(
    mut stream: std::net::TcpStream,
    responses: &[Value],
    used: &mut [bool],
    seen: &Mutex<Vec<String>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut start = String::new();
    reader.read_line(&mut start)?;
    let mut words = start.split_whitespace();
    let method = words.next().unwrap_or_default().to_string();
    let path = words.next().unwrap_or_default().to_string();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();
    if let Ok(mut seen) = seen.lock() {
        seen.push(format!("{method} {path} {body}"));
    }
    let matches = |r: &Value| {
        r["method"]
            .as_str()
            .unwrap_or("GET")
            .eq_ignore_ascii_case(&method)
            && r["path"]
                .as_str()
                .is_some_and(|p| path == p || path.starts_with(&format!("{p}?")))
            && r["match"].as_str().is_none_or(|m| body.contains(m))
    };
    // The first unused answer that matches, else the last that matches, so
    // a poll can be answered again.
    let index = responses
        .iter()
        .enumerate()
        .position(|(i, r)| !used[i] && matches(r))
        .or_else(|| responses.iter().rposition(matches));
    let (status, text, headers) = match index {
        Some(i) => {
            used[i] = true;
            let r = &responses[i];
            let text = match &r["body"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let mut headers = String::new();
            for (name, value) in r["headers"].as_object().into_iter().flatten() {
                headers.push_str(&format!(
                    "{name}: {}\r\n",
                    value.as_str().unwrap_or_default()
                ));
            }
            (r["status"].as_u64().unwrap_or(200), text, headers)
        }
        None => (
            501,
            json!({ "message": format!("the fixture records no response for {method} {path}") })
                .to_string(),
            String::new(),
        ),
    };
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routine::Origin;

    #[test]
    fn every_built_in_routine_passes_its_examples() {
        let catalog = Catalog::new(crate::routine::builtin());
        let logs = tempfile::tempdir().unwrap();
        for routine in catalog.routines() {
            let report = test(&catalog, routine, logs.path());
            assert!(
                report.ok,
                "{}",
                serde_json::to_string_pretty(&report).unwrap()
            );
        }
    }

    #[test]
    fn a_failing_example_says_what_was_expected() {
        let routine = Routine::load(crate::routine::Source {
            name: "hi".into(),
            origin: Origin::User(PathBuf::from("/r/hi.luau")),
            code: r#"--- about: hi
return {
    run = function() print("hello"); return { n = 1 }, 3 end,
    examples = {
        { name = "wrong", exit = 0, output = "bye", json = { n = 2 } },
        { name = "right", exit = 3, output = "hello", json = { n = 1 } },
    },
}"#
            .into(),
            trusted: true,
        });
        let catalog = Catalog::new(Vec::new());
        let logs = tempfile::tempdir().unwrap();
        let report = test(&catalog, &routine, logs.path());
        assert!(!report.ok);
        let wrong = &report.examples[0];
        assert!(!wrong.ok);
        assert!(
            wrong.problems[0].starts_with("exited 3, expected 0"),
            "{:?}",
            wrong.problems
        );
        assert!(wrong.problems[1].starts_with("the output lacks `bye`"));
        assert!(wrong.problems[2].starts_with("the JSON has 1 at $.n, expected 2"));
        assert!(report.examples[1].ok, "{:?}", report.examples[1].problems);
    }

    #[test]
    fn every_fixture_file_is_embedded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("routines/fixtures");
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = format!("fixtures/{}", entry.unwrap().file_name().to_string_lossy());
            assert!(
                FIXTURES.iter().any(|(path, _)| *path == name),
                "{name} is not in FIXTURES"
            );
        }
    }

    #[test]
    fn subsets_name_what_differs() {
        let actual = json!({ "prs": [{ "action": "conflict", "number": 7 }], "n": 1 });
        let mut path = "$".to_string();
        assert!(
            contains(
                &actual,
                &json!({ "prs": [{ "action": "conflict" }] }),
                &mut path
            )
            .is_ok()
        );
        let mut path = "$".to_string();
        assert_eq!(
            contains(
                &actual,
                &json!({ "prs": [{ "action": "draft" }] }),
                &mut path
            )
            .unwrap_err(),
            "has \"conflict\" at $.prs[0].action, expected \"draft\""
        );
        let mut path = "$".to_string();
        assert!(
            contains(&actual, &json!({ "prs": [] }), &mut path)
                .unwrap_err()
                .contains("expected 0")
        );
        let mut path = "$".to_string();
        assert!(contains(&json!({ "prs": [] }), &json!({ "prs": {} }), &mut path).is_ok());
    }
}
