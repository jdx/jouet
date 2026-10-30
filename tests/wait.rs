//! `jouet wait` on real files, ports, processes and an HTTP server, run as an
//! agent runs it: a subprocess, a result on stdout, and an exit code from the
//! shared table (0 met, 124 timeout, 2 usage).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const JOUET: &str = env!("CARGO_BIN_EXE_jouet");

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(JOUET);
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_LOG_DIR", dir.join("logs"))
        .env_remove("JOUET_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn jouet(dir: &Path, args: &[&str]) -> Output {
    command(dir, args).output().expect("jouet should start")
}

/// Starts a wait in the background, so the test can change the world under
/// it.
fn spawn(dir: &Path, args: &[&str]) -> Child {
    command(dir, args).spawn().expect("jouet should start")
}

fn finish(child: Child) -> Output {
    child.wait_with_output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(out)))
}

fn later(f: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1500));
        f();
    })
}

fn path_arg(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn a_file_that_appears_is_met() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ready");
    let child = spawn(
        dir.path(),
        &[
            "wait",
            "file",
            &path_arg(&path),
            "--interval",
            "1s",
            "--timeout",
            "20s",
            "--display",
            "agent",
        ],
    );
    let p = path.clone();
    later(move || fs::write(p, "x").unwrap()).join().unwrap();
    let out = finish(child);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).starts_with("MET file "), "{}", stdout(&out));
    // Agent mode prints each state once, not every check.
    let err = stderr(&out);
    assert_eq!(err.matches(": missing").count(), 1, "{err}");
    assert_eq!(err.matches(": exists").count(), 1, "{err}");
}

#[test]
fn a_timeout_exits_124_with_a_json_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_arg(&dir.path().join("never"));
    let out = jouet(
        dir.path(),
        &[
            "wait",
            "file",
            &path,
            "--timeout",
            "1s",
            "--interval",
            "1s",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(124));
    let doc = json(&out);
    assert_eq!(doc["result"], "timeout");
    assert_eq!(doc["state"], "missing");
    assert_eq!(doc["subject"], "file");
    assert!(doc["checks"].as_u64().unwrap() >= 2, "{doc}");
}

#[test]
fn file_contains_prints_the_matching_line_and_gone_waits_for_removal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("status.txt");
    fs::write(&path, "starting\n").unwrap();
    let child = spawn(
        dir.path(),
        &[
            "wait",
            "file",
            &path_arg(&path),
            "--contains",
            "^listening on \\d+",
            "--interval",
            "1s",
            "--json",
        ],
    );
    let p = path.clone();
    later(move || fs::write(p, "starting\nlistening on 8080\n").unwrap())
        .join()
        .unwrap();
    let out = finish(child);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(json(&out)["line"], "listening on 8080");

    let child = spawn(
        dir.path(),
        &[
            "wait",
            "file",
            &path_arg(&path),
            "--gone",
            "--interval",
            "1s",
        ],
    );
    let p = path.clone();
    later(move || fs::remove_file(p).unwrap()).join().unwrap();
    let out = finish(child);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

#[test]
fn a_log_wait_starts_at_the_end_and_follows_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    fs::write(&path, "server ready\n").unwrap();
    let child = spawn(
        dir.path(),
        &[
            "wait",
            "log",
            &path_arg(&path),
            "--match",
            "ready",
            "--interval",
            "1s",
            "--display",
            "plain",
        ],
    );
    let (p, d) = (path.clone(), dir.path().to_path_buf());
    later(move || {
        // Rotated away, then a new file with the line we want.
        fs::rename(&p, d.join("app.log.1")).unwrap();
        fs::write(&p, "booting\nserver ready again\n").unwrap();
    })
    .join()
    .unwrap();
    let out = finish(child);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(
        stdout(&out).lines().nth(1),
        Some("  server ready again"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_log_wait_from_the_start_reads_what_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    fs::write(&path, "one\nserver ready\n").unwrap();
    let args = [
        "wait",
        "log",
        &path_arg(&path),
        "--match",
        "ready",
        "--timeout",
        "0",
        "--json",
    ];
    let out = jouet(dir.path(), &args);
    assert_eq!(out.status.code(), Some(124), "the existing line is skipped");
    let mut args = args.to_vec();
    args.push("--from-start");
    let out = jouet(dir.path(), &args);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["line"], "server ready");
}

#[test]
fn a_port_opens_and_closes() {
    let dir = tempfile::tempdir().unwrap();
    // A port that is free now.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string();
    let addr = format!("127.0.0.1:{port}");
    let child = spawn(
        dir.path(),
        &["wait", "port", &addr, "--interval", "1s", "--json"],
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let a = addr.clone();
    later(move || tx.send(TcpListener::bind(a).unwrap()).unwrap())
        .join()
        .unwrap();
    let listener = rx.recv().unwrap();
    let out = finish(child);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(json(&out)["state"], "open");

    // A bare port means localhost.
    let out = jouet(dir.path(), &["wait", "port", &port, "--timeout", "0"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );

    let child = spawn(
        dir.path(),
        &["wait", "port", &addr, "--closed", "--interval", "1s"],
    );
    later(move || drop(listener)).join().unwrap();
    let out = finish(child);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

/// Answers every request with 503 until `ready` is set, then with 200 and a
/// body. Returns the base URL and how many requests were served.
fn http_server(ready: Arc<AtomicU32>) -> (String, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/health", listener.local_addr().unwrap());
    let served = Arc::new(AtomicU32::new(0));
    let count = served.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            count.fetch_add(1, Ordering::SeqCst);
            let (status, body) = if ready.load(Ordering::SeqCst) == 1 {
                ("200 OK", "status: starting\nstatus: ready\n")
            } else {
                ("503 Service Unavailable", "down\n")
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, served)
}

#[test]
fn http_waits_for_the_status_and_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let ready = Arc::new(AtomicU32::new(0));
    let (url, served) = http_server(ready.clone());
    let child = spawn(
        dir.path(),
        &[
            "wait",
            "http",
            &url,
            "--contains",
            "ready$",
            "--interval",
            "1s",
            "--jsonl",
        ],
    );
    later(move || ready.store(1, Ordering::SeqCst))
        .join()
        .unwrap();
    let out = finish(child);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let lines: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["event"], "state");
    assert_eq!(lines[0]["state"], "status 503");
    let result = lines.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["status"], 200);
    assert_eq!(result["line"], "status: ready");
    assert!(served.load(Ordering::SeqCst) >= 2);

    // An explicit --status is matched exactly.
    let out = jouet(
        dir.path(),
        &["wait", "http", &url, "--status", "503", "--timeout", "0"],
    );
    assert_eq!(out.status.code(), Some(124), "{}", stdout(&out));
}

#[test]
fn a_pid_is_met_when_the_process_exits() {
    let dir = tempfile::tempdir().unwrap();
    // jouet itself is the portable sleeping process: this wait times out.
    let missing = path_arg(&dir.path().join("never"));
    let mut sleeper = Command::new(JOUET)
        .args([
            "wait",
            "file",
            &missing,
            "--timeout",
            "2s",
            "--interval",
            "1s",
            "--quiet",
        ])
        .spawn()
        .unwrap();
    let pid = sleeper.id().to_string();
    // Reap it as soon as it exits, so it does not linger as a zombie.
    let reaper = thread::spawn(move || sleeper.wait().unwrap());
    let start = Instant::now();
    let out = jouet(
        dir.path(),
        &["wait", "pid", &pid, "--interval", "1s", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(json(&out)["state"], "exited");
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert_eq!(reaper.join().unwrap().code(), Some(124));
}

#[test]
fn a_command_is_rerun_until_it_exits_as_expected() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("flag");
    let flag_arg = path_arg(&flag);
    let child = spawn(
        dir.path(),
        &[
            "wait",
            "cmd",
            "--interval",
            "1s",
            "--json",
            "--",
            JOUET,
            "wait",
            "file",
            &flag_arg,
            "--timeout",
            "0",
        ],
    );
    later(move || fs::write(flag, "").unwrap()).join().unwrap();
    let out = finish(child);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json(&out);
    assert_eq!(doc["exit_code"], 0);
    assert!(doc["checks"].as_u64().unwrap() >= 2, "{doc}");
    // The output tail is the last run's, stdout and stderr together.
    let output: Vec<&str> = doc["output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(output.iter().any(|l| l.starts_with("MET file")), "{doc}");
    let log = PathBuf::from(doc["log_path"].as_str().unwrap());
    let mut saved = String::new();
    fs::File::open(log)
        .unwrap()
        .read_to_string(&mut saved)
        .unwrap();
    assert!(saved.contains("MET file"), "{saved}");
}

#[test]
fn a_command_that_cannot_start_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        dir.path(),
        &["wait", "cmd", "--json", "--", "jouet-no-such-command"],
    );
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["error"]["code"], "not_found");
}

#[test]
fn bad_requests_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["wait", "--json", "--jsonl", "pid", "1"][..],
        &["wait", "--timeout", "soon", "pid", "1"],
        &["wait", "--interval", "0", "pid", "1"],
        &["wait", "file", "x", "--gone", "--contains", "y"],
        &["wait", "log", "x", "--match", "("],
        &["wait", "port", "localhost"],
        &["wait", "http", "localhost:80"],
        &["wait", "pid", "0"],
        &["wait", "cmd"],
    ] {
        let out = jouet(dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}
