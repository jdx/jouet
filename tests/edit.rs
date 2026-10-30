//! `jouet edit` as an agent runs it: a spec on stdin, a result on stdout, and
//! an exit code from the shared table.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn jouet(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jouet"))
        .args(args)
        .current_dir(dir)
        .env("JOUET_LOG_DIR", dir.join("logs"))
        .env_remove("JOUET_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("jouet should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(out)))
}

fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        fs::write(dir.path().join(name), content).unwrap();
    }
    dir
}

const TWO_FILES: &str = "=== a.rs\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n=== b.rs\n<<<<<<< SEARCH\nmissing\n=======\nx\n>>>>>>> REPLACE\n";

#[test]
fn a_spec_on_stdin_is_applied() {
    let dir = fixture(&[("a.rs", "one\ntwo\n")]);
    let out = jouet(
        dir.path(),
        &["edit", "--display", "agent"],
        "=== a.rs\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "1\ntwo\n"
    );
    assert_eq!(
        stdout(&out),
        "ok          a.rs  1 edit  +1 -1\n1 of 1 file written\n"
    );
}

#[test]
fn a_failed_match_writes_nothing_and_exits_precondition() {
    let dir = fixture(&[("a.rs", "one\n"), ("b.rs", "two\n")]);
    let out = jouet(dir.path(), &["edit", "--json"], TWO_FILES);
    assert_eq!(out.status.code(), Some(4));
    let doc = json(&out);
    assert_eq!(doc["files_written"], 0);
    assert!(
        doc.get("applied").is_none(),
        "false fields are left out: {doc}"
    );
    assert_eq!(doc["files"][0]["status"], "not_written");
    assert_eq!(doc["files"][1]["edits"][0]["status"], "no_match");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one\n"
    );
}

#[test]
fn a_missing_file_exits_not_found() {
    let dir = fixture(&[]);
    let out = jouet(
        dir.path(),
        &["edit"],
        "=== nope.rs\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n",
    );
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
}

#[test]
fn a_spec_cut_short_is_a_usage_error_in_json_too() {
    let dir = fixture(&[("a.rs", "one\n")]);
    let out = jouet(
        dir.path(),
        &["edit", "--json"],
        "=== a.rs\n<<<<<<< SEARCH\none\n=======\n",
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "usage");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one\n"
    );
}

#[test]
fn a_failing_check_reverts_and_exits_failed() {
    let dir = fixture(&[("a.rs", "one\n")]);
    let check = if cfg!(windows) {
        "echo broken & exit /b 3"
    } else {
        "echo broken; exit 3"
    };
    let out = jouet(
        dir.path(),
        &["edit", "--json", "--check", check, "--revert-on-fail"],
        "=== a.rs\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["check"]["exit_code"], 3);
    assert_eq!(doc["check"]["reverted"], true);
    assert_eq!(doc["files"][0]["status"], "reverted");
    let log = doc["check"]["log_path"].as_str().unwrap();
    assert!(fs::read_to_string(log).unwrap().contains("broken"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one\n"
    );
}

#[test]
fn a_dry_run_reports_without_writing() {
    let dir = fixture(&[("a.rs", "one\n")]);
    let out = jouet(
        dir.path(),
        &["edit", "--dry-run", "--json"],
        "=== a.rs\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["dry_run"], true);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one\n"
    );
}

#[test]
fn replace_and_hash_guard_a_stale_file() {
    let dir = fixture(&[("a.rs", "one two\n")]);
    let out = jouet(dir.path(), &["edit", "hash", "a.rs", "--json"], "");
    let hash = json(&out)["files"][0]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    let guard = format!("a.rs={hash}");

    let out = jouet(
        dir.path(),
        &[
            "edit",
            "replace",
            "a.rs",
            "--old",
            "two",
            "--new",
            "2",
            "--expect-hash",
            &guard,
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one 2\n"
    );

    // The same guard is now stale.
    let out = jouet(
        dir.path(),
        &[
            "edit",
            "replace",
            "a.rs",
            "--old",
            "one",
            "--new",
            "1",
            "--expect-hash",
            &guard,
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one 2\n"
    );
}

#[test]
fn a_codex_patch_is_accepted_as_is() {
    let dir = fixture(&[("a.rs", "x\ny\n")]);
    let out = jouet(
        dir.path(),
        &["edit"],
        "*** Begin Patch\n*** Update File: a.rs\n x\n-y\n+z\n*** Add File: b.rs\n+new\n*** End Patch\n",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "x\nz\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.rs")).unwrap(),
        "new\n"
    );
}

#[test]
fn relative_and_absolute_paths_to_one_file_compose() {
    let dir = fixture(&[("a.rs", "one two\n")]);
    let abs = dir.path().join("a.rs");
    let spec = format!(
        "=== a.rs\n<<<<<<< SEARCH\none two\n=======\n1 two\n>>>>>>> REPLACE\n=== {}\n<<<<<<< SEARCH\n1 two\n=======\n1 2\n>>>>>>> REPLACE\n",
        abs.display()
    );
    let out = jouet(dir.path(), &["edit"], &spec);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(fs::read_to_string(abs).unwrap(), "1 2\n");
}

#[test]
fn a_check_that_cannot_start_still_reverts() {
    let dir = fixture(&[("a.rs", "one\n"), ("not-a-dir", "")]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_jouet"))
        .args(["edit", "--json", "--check", "true", "--revert-on-fail"])
        .current_dir(dir.path())
        // A regular file where the log directory should be, so the check
        // cannot write its log and never runs.
        .env("JOUET_LOG_DIR", dir.path().join("not-a-dir"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"=== a.rs\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["error"]["code"], "failed");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("restored"),
        "{doc}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one\n"
    );
}
