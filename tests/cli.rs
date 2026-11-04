//! The binary's command-line surface, exercised as a subprocess.

use std::process::Command;

fn jouet(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_jouet"))
        .args(args)
        .output()
        .expect("jouet should start")
}

#[test]
fn version_names_the_package_version() {
    let out = jouet(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        concat!("jouet ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_succeeds() {
    let out = jouet(&["--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: jouet"));
}

// Exit code 2 is `usage` in jouet's shared exit-code table.
#[test]
fn unknown_flag_is_a_usage_error() {
    let out = jouet(&["--no-such-flag"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn json_parse_errors_are_one_structured_document() {
    for args in [
        vec!["init", "--json"],
        vec!["--json", "--no-such-flag"],
        vec!["test", "--json", "--since"],
    ] {
        let out = jouet(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["error"]["code"], "usage", "{args:?}");
        assert!(!json["error"]["message"].as_str().unwrap().is_empty());
        assert!(out.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn help_with_json_still_prints_help() {
    let out = jouet(&["--json", "--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: jouet"));
}

#[test]
fn valid_json_command_still_prints_one_document() {
    let out = jouet(&["--json", "rule", "schema"]);
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.is_object());
}

#[test]
fn json_after_double_dash_is_not_a_global_option() {
    let out = jouet(&["--no-such-flag", "--", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--no-such-flag"));
}
