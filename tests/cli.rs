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

#[cfg(unix)]
#[test]
fn pnpm_vitest_cost_is_recorded_only_when_not_shadowed_by_a_script() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let pnpm = bin.join("pnpm");
    std::fs::write(
        &pnpm,
        "#!/bin/sh\nprintf 'Test Files  1 passed (1)\\n     Tests  1 passed (1)\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&pnpm, std::fs::Permissions::from_mode(0o755)).unwrap();
    let manifest = dir.join("package.json");
    std::fs::write(&manifest, r#"{"scripts":{"test":"vitest run"}}"#).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    git(&["init", "-q"]);
    git(&["add", "package.json"]);
    git(&["commit", "-qm", "base"]);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |file: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_jouet"))
            .current_dir(dir)
            .args(["test", "--cmd", "--", "pnpm", "vitest", "run", file])
            .env("PATH", &path)
            .env("HOME", dir)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env_remove("JOUET_COST_RECORD")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run("one.test.ts");
    let note = git(&["notes", "--ref", "jouet-cost", "show", "HEAD"]);
    assert!(String::from_utf8_lossy(&note).contains("pnpm vitest run one.test.ts"));
    std::fs::write(&manifest, r#"{"scripts":{"vitest":"echo not a test"}}"#).unwrap();
    run("two.test.ts");
    assert_eq!(git(&["notes", "--ref", "jouet-cost", "show", "HEAD"]), note);
}

#[cfg(unix)]
#[test]
fn npx_jest_cost_is_recorded() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let npx = bin.join("npx");
    std::fs::write(&npx, "#!/bin/sh\nprintf 'Tests: 1 passed, 1 total\\n'\n").unwrap();
    std::fs::set_permissions(&npx, std::fs::Permissions::from_mode(0o755)).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    git(&["init", "-q"]);
    git(&["commit", "--allow-empty", "-qm", "base"]);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_jouet"))
        .current_dir(dir)
        .args([
            "test",
            "--cmd",
            "--",
            "npx",
            "jest",
            "tests/example.test.js",
        ])
        .env("PATH", path)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env_remove("JOUET_COST_RECORD")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let note = git(&["notes", "--ref", "jouet-cost", "show", "HEAD"]);
    assert!(String::from_utf8_lossy(&note).contains("npx jest tests/example.test.js"));
}
