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
