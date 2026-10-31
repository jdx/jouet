//! `jouet wind memories` against a local fake GitHub: what saved memories
//! wait on is looked up there, never on the real API.

mod common;

use common::{Fake, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output, Stdio};

const TOKEN: &str = "good-token";

/// A home with one Claude Code project's memories: one waiting for a
/// release, one for a merge, and one that names no PR at all.
fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let memory = dir.path().join(".claude/projects/-work-app/memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("MEMORY.md"), "- [mbx GC](mbx-gc.md)\n").unwrap();
    std::fs::write(
        memory.join("mbx-gc.md"),
        "---\nname: mbx-gc\ndescription: run cargo through mise x\n---\n\n\
         Run cargo through `mise x -- cargo`.\n\n\
         **When to drop this:** jdx/mbx#886 (merged 2026-10-31) keeps the target. \
         It isn't in a release yet.\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("pin.md"),
        "---\nname: pin-tool\ndescription: pin the tool\n---\n\nPin it until jdx/tool#12 merges. Blocked on jdx/tool#99 too.\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("plain.md"),
        "---\nname: plain\ndescription: a preference\n---\n\nKeep PR titles short.\n",
    )
    .unwrap();
    dir
}

fn jouet(fake: &Fake, home: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", TOKEN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "JOUET_DISPLAY",
        "JOUET_WIND",
        "JOUET_WIND_MEMORIES",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "JOUET_HOOKS",
    ] {
        cmd.env_remove(var);
    }
    let mut child = cmd.spawn().unwrap();
    if let Some(text) = stdin {
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

/// GitHub's answers: both PRs merged, and mbx's latest release has #886.
fn respond(request: &Request) -> (u16, String) {
    if request.method == "GET" && request.path == "/repos/jdx/mbx/compare/v1.30.0...c886" {
        return (200, json!({"status": "behind"}).to_string());
    }
    let q = query(request);
    if !q.contains("issueOrPullRequest") {
        return (404, json!({"message": "Not Found"}).to_string());
    }
    let pr = |n: &str, at: &str| json!({"__typename": "PullRequest", "state": "MERGED", "mergedAt": at, "mergeCommit": {"oid": n}});
    let body = json!({"data": {
        "r0": {"latestRelease": {"tagName": "v1.30.0", "publishedAt": "2026-11-02T10:00:00Z"}, "n886": pr("c886", "2026-10-31T09:00:00Z")},
        "r1": {"latestRelease": null, "n12": pr("c12", "2026-11-01T09:00:00Z")}
    }});
    (200, body.to_string())
}

#[test]
fn memories_waiting_on_merged_and_released_prs_become_notes() {
    let fake = serve(|request, _| respond(request));
    let home = home();
    let out = jouet(
        &fake,
        home.path(),
        &["wind", "memories", "--record", "--json"],
        None,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["memories"], 3);
    // jdx/tool#99 does not exist: remembered as such, and not asked again.
    assert_eq!(doc["asked"], 3);
    let found: Vec<(&str, &str)> = doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["memory"].as_str().unwrap(), f["found"].as_str().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "mbx-gc",
                "waits for a release with jdx/mbx#886, and v1.30.0 (2026-11-02) has it"
            ),
            (
                "pin-tool",
                "waits on PR jdx/tool#12, which was merged on 2026-11-01"
            ),
        ]
    );
    assert_eq!(doc["recorded"], json!(["m1", "m2"]));
    let requests = fake.requests();
    assert!(requests.iter().all(|r| r.token == TOKEN));
    assert_eq!(requests.len(), 2, "{requests:?}");

    // Again: GitHub's answers are settled, so nothing is asked, and nothing
    // is noted twice.
    let out = jouet(&fake, home.path(), &["wind", "memories", "--json"], None);
    let doc = json(&out);
    assert_eq!(doc["asked"], 0);
    assert_eq!(doc["findings"].as_array().unwrap().len(), 2);
    assert_eq!(doc.get("recorded"), None);
    assert_eq!(fake.requests().len(), 2);

    // A session starting in the project sees both, one line each.
    let payload =
        json!({"hook_event_name": "SessionStart", "session_id": "s1", "cwd": "/work/app"});
    let out = jouet(
        &fake,
        home.path(),
        &["hook", "claude", "session-start"],
        Some(&payload.to_string()),
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let context = doc["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("jouet wind: memory `mbx-gc` waits for a release with jdx/mbx#886"),
        "{context}"
    );
    assert!(context.contains("`jouet wind dismiss m2`"), "{context}");

    // `jouet wind show` gives the path to change, after asking the user.
    let out = jouet(&fake, home.path(), &["wind", "show", "m1"], None);
    assert!(
        stdout(&out).contains("path: ~/.claude/projects/-work-app/memory/mbx-gc.md"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn without_github_the_rest_is_still_checked() {
    let fake = serve(|_, _| (500, json!({"message": "down"}).to_string()));
    let home = home();
    std::fs::write(
        home.path().join(".claude/projects/-work-app/memory/old.md"),
        "---\nname: old\n---\n\nRun `jouet frobnicate --now` first.\n",
    )
    .unwrap();
    let out = jouet(&fake, home.path(), &["wind", "memories", "--json"], None);
    assert!(out.status.success());
    let doc = json(&out);
    assert!(doc["github"].as_str().unwrap().contains("500"), "{doc}");
    assert_eq!(
        doc["findings"][0]["found"],
        "names `jouet frobnicate`, which this jouet does not have"
    );
}
