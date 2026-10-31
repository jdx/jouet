use super::*;
use std::time::{Duration, SystemTime};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn age(path: &Path, secs: u64) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[test]
fn reads_where_each_session_last_worked() {
    let dir = tempfile::tempdir().unwrap();
    let claude = ClaudeCode {
        root: dir.path().join("claude"),
    };
    let codex = Codex {
        root: dir.path().join("codex"),
    };
    // Claude Code: the latest line with a `cwd` wins, metadata lines after
    // it are passed over, and a subagent's transcript is not a session.
    write(
        &claude.root.join("projects/-w-a/aaaa-1111.jsonl"),
        concat!(
            r#"{"type":"user","cwd":"/w/a","gitBranch":"main","message":{"content":"secret plan"}}"#,
            "\n",
            r#"{"type":"assistant","cwd":"/w/a/sub","gitBranch":"feat/x","message":{}}"#,
            "\n",
            r#"{"type":"bridge-session","sessionId":"aaaa-1111"}"#,
            "\n",
        ),
    );
    write(
        &claude
            .root
            .join("projects/-w-a/aaaa-1111/subagents/agent-1.jsonl"),
        "{\"type\":\"user\",\"cwd\":\"/w/a\"}\n",
    );
    // An old one, outside the window.
    let old = claude.root.join("projects/-w-b/bbbb-2222.jsonl");
    write(&old, "{\"type\":\"user\",\"cwd\":\"/w/b\"}\n");
    age(&old, 3 * 24 * 3600);
    // Codex: `session_meta` first, then a `turn_context` that moved.
    write(
        &codex
            .root
            .join("sessions/2026/10/31/rollout-2026-10-31T10-00-00-cccc.jsonl"),
        concat!(
            r#"{"type":"session_meta","payload":{"id":"cccc-3333","cwd":"/w/c","git":{"branch":"fix/y"},"base_instructions":{"text":"long"}}}"#,
            "\n",
            r#"{"type":"turn_context","payload":{"cwd":"/w/c/deeper"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
            "\n",
        ),
    );
    write(
        &codex.root.join("sessions/2026/10/31/notes.jsonl"),
        "{\"type\":\"session_meta\"}\n",
    );

    let since = now_ms() - 24 * 3600 * 1000;
    let sessions = recent(Some(&claude), Some(&codex), since);
    let got: Vec<_> = sessions
        .iter()
        .map(|s| {
            (
                s.harness,
                s.id.as_str(),
                s.cwd.as_deref(),
                s.branch.as_deref(),
            )
        })
        .collect();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.contains(&("claude", "aaaa-1111", Some("/w/a/sub"), Some("feat/x"))));
    assert!(got.contains(&("codex", "cccc-3333", Some("/w/c/deeper"), Some("fix/y"))));
}

#[test]
fn only_the_end_of_a_long_transcript_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let mut text = String::new();
    text.push_str("{\"type\":\"user\",\"cwd\":\"/early\"}\n");
    let filler = format!("{{\"type\":\"x\",\"pad\":\"{}\"}}\n", "z".repeat(1000));
    for _ in 0..100 {
        text.push_str(&filler);
    }
    write(&path, &text);
    let found = |l: &Value| l.get("cwd").is_some();
    assert_eq!(last_line(&path, b"\"cwd\"", found), None);
    assert_eq!(
        first_line(&path, TAIL, found).and_then(|l| text_of(&l)),
        Some("/early".into())
    );
}

fn text_of(line: &Value) -> Option<String> {
    text(line, &["cwd"])
}

#[test]
fn head_names_the_branch_of_a_checkout_or_a_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    write(&main.join(".git/HEAD"), "ref: refs/heads/main\n");
    assert_eq!(head_branch(&main), Some("main".into()));

    let linked = dir.path().join("linked");
    let git_dir = main.join(".git/worktrees/linked");
    write(&git_dir.join("HEAD"), "0123456789abcdef0123\n");
    write(
        &linked.join(".git"),
        &format!("gitdir: {}\n", git_dir.display()),
    );
    assert_eq!(head_branch(&linked), Some("0123456789ab".into()));
    assert_eq!(head_branch(&dir.path().join("none")), None);
}
