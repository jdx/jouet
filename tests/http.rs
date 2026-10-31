//! `jouet http` against a local server, run as an agent runs it: a
//! subprocess, the result on stdout, and an exit code from the shared table.
//! Nothing leaves the machine.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

const JOUET: &str = env!("CARGO_BIN_EXE_jouet");

fn jouet(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(JOUET);
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_LOG_DIR", dir.join("logs"))
        .env_remove("JOUET_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The test's server must be reached directly.
    for var in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("jouet should start")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn both(out: &Output) -> String {
    format!("{}{}", stdout(out), String::from_utf8_lossy(&out.stderr))
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", both(out)))
}

fn items() -> String {
    let items: Vec<String> = (1..=50)
        .map(|i| format!(r#"{{"id":{i},"name":"item {i}"}}"#))
        .collect();
    format!(
        r#"{{"ok":true,"access_token":"tok-value-xyz","items":[{}]}}"#,
        items.join(",")
    )
}

/// A server with a few routes. `/echo` answers with the request it read;
/// `/ready` says `ready: true` from its third request on.
fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let readies = Arc::new(AtomicU32::new(0));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let readies = readies.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut headers = Vec::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let line = line.trim_end().to_string();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    headers.push(line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let target = first.split_whitespace().nth(1).unwrap_or("/");
                let path = target.split('?').next().unwrap_or("/").to_string();
                let (status, extra, body): (&str, &str, Vec<u8>) = match path.as_str() {
                    "/items" => ("200 OK", "", items().into_bytes()),
                    "/echo" => {
                        let echo = serde_json::json!({
                            "request": first.trim_end(),
                            "headers": headers,
                            "body": String::from_utf8_lossy(&body),
                        });
                        ("200 OK", "", echo.to_string().into_bytes())
                    }
                    "/missing" => ("404 Not Found", "", br#"{"message":"Not Found"}"#.to_vec()),
                    "/moved" => ("302 Found", "location: /items\r\n", Vec::new()),
                    "/binary" => ("200 OK", "", vec![0, 159, 146, 150, 255]),
                    "/slow" => {
                        thread::sleep(Duration::from_secs(3));
                        ("200 OK", "", b"{}".to_vec())
                    }
                    "/ready" => {
                        let n = readies.fetch_add(1, Ordering::SeqCst);
                        let body = format!(r#"{{"ready":{}}}"#, n >= 2);
                        ("200 OK", "", body.into_bytes())
                    }
                    _ => ("404 Not Found", "", b"no route\n".to_vec()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            });
        }
    });
    base
}

#[test]
fn selected_paths_print_only_their_values_within_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/items", server());
    let out = jouet(
        dir.path(),
        &[
            "http",
            "GET",
            &url,
            "-s",
            "$.ok",
            "-s",
            "$.items[?@.id > 48].name",
            "-s",
            "$.items[*].id",
            "-s",
            "$.access_token",
            "-s",
            "$.nope",
            "--budget",
            "40",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with("HTTP 200 OK, application/json, "),
        "{text}"
    );
    assert_eq!(lines[1], "$.ok: true");
    assert_eq!(lines[2], "$.items[?@.id > 48].name: 2 matches");
    assert_eq!(lines[3], r#"  "item 49""#);
    assert_eq!(lines[4], r#"  "item 50""#);
    assert_eq!(lines[5], "$.items[*].id: 50 matches");
    // The budget is spent on the first matches; the rest are counted.
    let marker = lines
        .iter()
        .find(|l| l.contains("more matches (full body: "))
        .unwrap_or_else(|| panic!("{text}"));
    let saved = marker
        .split("full body: ")
        .nth(1)
        .unwrap()
        .trim_end_matches(')');
    let saved = std::fs::read_to_string(saved).unwrap();
    assert!(saved.contains("\"item 50\""), "{saved}");
    assert!(
        !saved.contains("tok-value-xyz"),
        "the saved body is redacted"
    );
    assert!(
        text.contains("$.access_token: 1 match not shown, over --budget (full body: "),
        "{text}"
    );
    assert!(text.ends_with("$.nope: no match\n"), "{text}");
    assert!(!text.contains("tok-value-xyz"), "{text}");
}

#[test]
fn a_long_body_is_cut_with_a_marker_naming_the_saved_file() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/items", server());
    let out = jouet(dir.path(), &["http", "GET", &url, "--budget", "100"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(lines[1].len(), 100, "{text}");
    assert!(
        lines[1].starts_with(r#"{"ok":true,"access_token":"[redacted]","items":"#),
        "{text}"
    );
    assert!(lines[2].starts_with("... "), "{text}");
    assert!(lines[2].contains(" more bytes (full body: "), "{text}");

    let out = jouet(
        dir.path(),
        &["--json", "http", "GET", &url, "--budget", "0"],
        &[],
    );
    let doc = json(&out);
    assert_eq!(doc["status"], 200);
    assert_eq!(doc["body"]["items"][49]["id"], 50);
    assert_eq!(doc["body"]["access_token"], "[redacted]");
    assert!(
        doc.get("body_text").is_none() && doc.get("omitted_bytes").is_none(),
        "{doc}"
    );
}

#[test]
fn assertions_pass_or_name_the_one_that_failed() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/items", server());
    let pass = [
        "status=200",
        "header content-type contains json",
        "jsonpath $.items count == 50",
        "jsonpath $.items[?@.id == 7].name == 'item 7'",
    ];
    let mut args = vec!["http", "GET", url.as_str(), "-s", "$.ok"];
    for a in &pass {
        args.extend(["--expect", a]);
    }
    let out = jouet(dir.path(), &args, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    assert!(
        stdout(&out).ends_with("expect: 4 passed\n"),
        "{}",
        both(&out)
    );

    let out = jouet(
        dir.path(),
        &[
            "http",
            "GET",
            &url,
            "-s",
            "$.ok",
            "-e",
            "jsonpath $.ok == false",
            "-e",
            "status=200",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "{}", both(&out));
    assert!(
        stdout(&out).ends_with("expect failed: jsonpath $.ok == false: got true\n"),
        "{}",
        both(&out)
    );

    let out = jouet(
        dir.path(),
        &[
            "--json",
            "http",
            "GET",
            &url,
            "-s",
            "$.ok",
            "-e",
            "jsonpath $.ok == false",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    let doc = json(&out);
    assert_eq!(doc["assertions"][0]["assertion"], "jsonpath $.ok == false");
    assert_eq!(doc["assertions"][0]["actual"], true);
    assert!(
        doc["assertions"][0].get("passed").is_none(),
        "false is left out: {doc}"
    );
}

#[test]
fn an_error_status_fails_unless_asked_for_and_shows_its_body() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/missing", server());
    let out = jouet(dir.path(), &["http", "GET", &url, "-s", "$.id"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", both(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\n{\"message\":\"Not Found\"}\n$.id: no match\n"),
        "{text}"
    );
    assert!(
        text.ends_with(
            "expect failed: status < 400: got 404; give --expect status=404 to accept it\n"
        ),
        "{text}"
    );
    let out = jouet(
        dir.path(),
        &["http", "GET", &url, "--expect", "status == 404"],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
}

#[test]
fn the_request_is_sent_as_given_and_credentials_never_printed() {
    let dir = tempfile::tempdir().unwrap();
    let base = server();
    // `${NAME}` is jouet's to expand, so the URL keeps it as given.
    let url = format!("{base}/echo?key=${{JOUET_TEST_KEY}}");
    let out = jouet(
        dir.path(),
        &[
            "--json",
            "http",
            "post",
            &url,
            "-H",
            "Authorization: Bearer literal-bearer-token",
            "-H",
            "X-Trace: ${JOUET_TEST_TRACE}",
            "--json-body",
            r#"{"name":"x","secret":"${JOUET_TEST_KEY}"}"#,
            "--budget",
            "0",
        ],
        &[
            ("JOUET_TEST_KEY", "key-value-1234"),
            ("JOUET_TEST_TRACE", "trace-5678"),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    let text = stdout(&out);
    for secret in ["key-value-1234", "trace-5678", "literal-bearer-token"] {
        assert!(!text.contains(secret), "{secret} printed: {text}");
    }
    let doc = json(&out);
    assert_eq!(doc["method"], "POST");
    assert_eq!(doc["url"], url);
    let body = &doc["body"];
    assert_eq!(body["request"], "POST /echo?key=[redacted] HTTP/1.1");
    let headers: Vec<&str> = body["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap())
        .collect();
    assert!(
        headers.contains(&"authorization: [redacted]"),
        "{headers:?}"
    );
    assert!(headers.contains(&"x-trace: [redacted]"), "{headers:?}");
    assert!(
        headers.contains(&"content-type: application/json"),
        "{headers:?}"
    );
    assert_eq!(body["body"], r#"{"name":"x","secret":"[redacted]"}"#);

    // `--data @FILE` sends the file as it is.
    std::fs::write(dir.path().join("body.txt"), "plain text").unwrap();
    let out = jouet(
        dir.path(),
        &[
            "--json",
            "http",
            "PUT",
            &format!("{base}/echo"),
            "--data",
            "@body.txt",
        ],
        &[],
    );
    assert_eq!(json(&out)["body"]["body"], "plain text", "{}", both(&out));
}

#[test]
fn redirects_are_followed_unless_asked_not_to() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/moved", server());
    let out = jouet(dir.path(), &["http", "GET", &url, "-s", "$.ok"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    assert!(stdout(&out).contains("$.ok: true"), "{}", both(&out));
    let out = jouet(
        dir.path(),
        &[
            "http",
            "GET",
            &url,
            "--no-follow",
            "--expect",
            "status == 302",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    let text = stdout(&out);
    assert!(text.starts_with("HTTP 302 Found, "), "{text}");
    assert!(text.contains("\nlocation: /items\n"), "{text}");
}

#[test]
fn a_binary_body_is_saved_rather_than_printed() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/binary", server());
    let out = jouet(dir.path(), &["http", "GET", &url], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    let text = stdout(&out);
    let line = text.lines().nth(1).unwrap();
    assert!(
        line.starts_with("(binary body, 5 bytes, saved to "),
        "{text}"
    );
    let path = line
        .split("saved to ")
        .nth(1)
        .unwrap()
        .trim_end_matches(')');
    assert_eq!(std::fs::read(path).unwrap(), [0, 159, 146, 150, 255]);
}

#[test]
fn refusals_and_failures_exit_with_the_shared_codes() {
    let dir = tempfile::tempdir().unwrap();
    let base = server();
    let refused = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/", l.local_addr().unwrap())
    };
    let items = format!("{base}/items");
    let slow = format!("{base}/slow");
    let unset = format!("{base}/${{JOUET_TEST_SURELY_UNSET}}");
    let cases: &[(&[&str], i32, &str)] = &[
        (&["http", "FETCH", &items], 2, "is not an HTTP method"),
        (
            &["http", "GET", "ftp://example.com"],
            2,
            "is not an http:// or https:// URL",
        ),
        (
            &["http", "GET", &items, "-H", "nocolon"],
            2,
            "is not NAME:VALUE",
        ),
        (
            &["http", "GET", &items, "-s", "items"],
            2,
            "is not a JSONPath",
        ),
        (
            &["http", "GET", &items, "-e", "stauts=200"],
            2,
            "starts with status",
        ),
        (
            &["http", "POST", &items, "--json-body", "{nope"],
            2,
            "--json-body is not JSON",
        ),
        (
            &["http", "POST", &items, "--data", "@missing.json"],
            3,
            "--data @missing.json",
        ),
        (
            &["http", "GET", &unset],
            4,
            "`${JOUET_TEST_SURELY_UNSET}` is not set",
        ),
        (&["http", "GET", &refused], 5, "GET http://"),
        (
            &["http", "GET", &slow, "--timeout", "1s"],
            124,
            "no response in time",
        ),
    ];
    for (args, code, says) in cases {
        let out = jouet(dir.path(), args, &[]);
        assert_eq!(out.status.code(), Some(*code), "{args:?}: {}", both(&out));
        assert!(both(&out).contains(says), "{args:?}: {}", both(&out));
        // With --json, one document, the error included.
        let mut with_json = vec!["--json"];
        with_json.extend_from_slice(args);
        let doc = json(&jouet(dir.path(), &with_json, &[]));
        assert!(
            doc["error"]["message"].as_str().unwrap().contains(says),
            "{doc}"
        );
    }
}

#[test]
fn wait_http_waits_until_its_assertions_hold() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/ready", server());
    let out = jouet(
        dir.path(),
        &[
            "wait",
            "http",
            &url,
            "--expect",
            "jsonpath $.ready == true",
            "--interval",
            "100ms",
            "--timeout",
            "20s",
            "--jsonl",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", both(&out));
    let lines: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        lines[0]["state"],
        "status 200, jsonpath $.ready == true: got false"
    );
    let result = lines.last().unwrap();
    assert_eq!(result["result"], "met");
    assert_eq!(result["state"], "status 200");
}
