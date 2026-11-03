//! Scheduled reviews of compressed Codex archives through the CLI.

use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

fn run(home: &Path, project: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_jouet"))
        .args(args)
        .arg("--json")
        .current_dir(project)
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("CODEX_THREAD_ID")
        .env_remove("JOUET_SESSION_ID")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn compressed_archive_can_be_claimed_reviewed_and_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let project = home.join("project");
    std::fs::create_dir_all(&project).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&project)
            .status()
            .unwrap()
            .success()
    );
    let archives = home.join(".codex/archived_sessions");
    std::fs::create_dir_all(&archives).unwrap();
    let transcript =
        archives.join("rollout-2026-01-01T00-00-00-01a00000-0000-7000-8000-000000000001.jsonl.zst");
    let id = "01a00000-0000-7000-8000-000000000001";
    let meta = json!({"type":"session_meta","payload":{"id":id,"cwd":project,"originator":"codex_cli_rs"}});
    let message = json!({"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run the tests"}]}});
    let contents = format!("{meta}\n{message}\n");
    std::fs::write(
        &transcript,
        zstd::stream::encode_all(contents.as_bytes(), 0).unwrap(),
    )
    .unwrap();
    let root = project.to_str().unwrap();
    let plan = run(
        &home,
        &project,
        &["wind", "plan", "--root", root, "--idle", "0s"],
    );
    let snapshot = plan["candidates"][0]["snapshot"].as_str().unwrap();
    let claim = run(
        &home,
        &project,
        &["wind", "claim", snapshot, "--root", root, "--idle", "0s"],
    );
    let lease = claim["lease"].as_str().unwrap();
    let path = claim["transcript"].as_str().unwrap();
    let review = run(
        &home,
        &project,
        &["wind", "review", path, "--project", root, "--lease", lease],
    );
    assert!(review["prompt"].as_str().unwrap().contains(".zst"));
    let reply = home.join("reply.json");
    std::fs::write(
        &reply,
        json!({"session":id,"proposals":[{"kind":"nothing"}]}).to_string(),
    )
    .unwrap();
    let result = run(
        &home,
        &project,
        &["wind", "propose", reply.to_str().unwrap(), "--lease", lease],
    );
    assert_eq!(result["scheduled_reviewed"], true);
    let plan = run(
        &home,
        &project,
        &["wind", "plan", "--root", root, "--idle", "0s"],
    );
    assert_eq!(plan["reviewed"], 1);
}
