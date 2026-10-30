//! `jouet commit`, `review-package`, `handoff` and `pickup`, run as a
//! subprocess against throwaway repositories. The environment is cleared,
//! so neither the developer's git config nor the agent running the tests
//! changes what the commands do.

mod common;

use common::{json, stderr, stdout};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(sandbox.repo()).unwrap();
        std::fs::create_dir_all(sandbox.dir.path().join("home")).unwrap();
        std::fs::write(sandbox.dir.path().join("gitconfig"), "").unwrap();
        sandbox.git(&["init", "-q", "-b", "main"]);
        sandbox.git(&["config", "core.hooksPath", ".git/hooks"]);
        sandbox
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn logs(&self) -> PathBuf {
        self.dir.path().join("logs")
    }

    fn env(&self, c: &mut Command) {
        let home = self.dir.path().join("home");
        c.env_clear()
            .current_dir(self.repo())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("JOUET_LOG_DIR", self.logs())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.dir.path().join("gitconfig"))
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com");
    }

    fn git(&self, args: &[&str]) -> String {
        let mut c = Command::new("git");
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
        stdout(&out).trim().to_string()
    }

    fn jouet(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_jouet"));
        self.env(&mut c);
        c.args(args).envs(vars.iter().copied()).output().unwrap()
    }

    fn write(&self, path: &str, text: &str) {
        let full = self.repo().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    fn commit(&self, path: &str, text: &str, message: &str) {
        self.write(path, text);
        self.git(&["add", "--", path]);
        self.git(&["commit", "-q", "-m", message]);
    }
}

const CLAUDE: (&str, &str) = ("AI_AGENT", "claude-code_2-1-309_agent");

#[test]
fn commit_takes_only_the_named_paths_and_adds_the_agent_trailers() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    s.write("a.txt", "a2\n");
    s.write("b.txt", "b\n");
    s.git(&["add", "b.txt"]);
    s.write("cache/blob", "x\n");

    let out = s.jouet(&["commit", "a.txt", "-m", "change a", "--json"], &[CLAUDE]);

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["committed"], true);
    assert_eq!(doc["files"][0]["path"], "a.txt");
    assert_eq!(
        s.git(&["log", "-1", "--format=%B"]),
        "change a\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n\
         AI-Assisted: Tool: Claude Code; model: unavailable; version: 2.1.309"
    );
    assert_eq!(
        s.git(&["status", "--porcelain", "--untracked-files=all"]),
        "A  b.txt\n?? cache/blob"
    );
}

#[test]
fn commit_trailers_come_from_the_environment_or_config() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");

    s.write("a.txt", "1\n");
    let trailers = "Co-Authored-By: Bot <bot@example.com>\nAI-Assisted: Tool: {tool}; model: x/y; version: {version}";
    let out = s.jouet(
        &["commit", "a.txt", "-m", "one"],
        &[CLAUDE, ("JOUET_COMMIT_TRAILERS", trailers)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        s.git(&["log", "-1", "--format=%(trailers)"]),
        "Co-Authored-By: Bot <bot@example.com>\n\
         AI-Assisted: Tool: Claude Code; model: x/y; version: 2.1.309"
    );

    // Outside an agent nothing is added by default; config can add some.
    s.write("a.txt", "2\n");
    let out = s.jouet(&["commit", "a.txt", "-m", "two"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(s.git(&["log", "-1", "--format=%B"]), "two");

    let config = s.dir.path().join("home/.config/jouet");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[commit]\ntrailers = [\"Signed-off-by: Test <test@example.com>\"]\n",
    )
    .unwrap();
    s.write("a.txt", "3\n");
    let out = s.jouet(
        &["commit", "a.txt", "-m", "three", "--trailer", "Refs: #7"],
        &[CLAUDE],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        s.git(&["log", "-1", "--format=%(trailers)"]),
        "Signed-off-by: Test <test@example.com>\nRefs: #7"
    );

    s.write("a.txt", "4\n");
    let out = s.jouet(
        &["commit", "a.txt", "-m", "four", "--no-trailers"],
        &[CLAUDE],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(s.git(&["log", "-1", "--format=%B"]), "four");
}

#[test]
fn commit_refuses_sweeping_flags_and_paths() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    s.write("a.txt", "a2\n");
    for args in [
        &["commit", "-A", "-m", "x"][..],
        &["commit", "--all", "-m", "x"],
        &["commit", ".", "-m", "x"],
        &["commit", "-m", "x"],
        &["commit", "a.txt", "--no-verify", "-m", "x"],
        &["commit", "a.txt"],
    ] {
        let out = s.jouet(args, &[]);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    let out = s.jouet(&["commit", "-A", "-m", "x", "--json"], &[]);
    let doc = json(&out);
    assert_eq!(doc["error"]["code"], "usage");
    assert!(
        doc["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("name the paths")
    );
    let out = s.jouet(&["commit", "b.txt", "-m", "x"], &[]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(s.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[cfg(unix)]
#[test]
fn commit_reports_a_failing_hook() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    let hook = s.repo().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho 'lint: a.txt is wrong'\nexit 3\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    s.write("a.txt", "a2\n");

    let out = s.jouet(&["commit", "a.txt", "-m", "change"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("commit failed (exit 1)"), "{text}");
    assert!(text.contains("  lint: a.txt is wrong\n"), "{text}");
    let log = text
        .lines()
        .find_map(|l| l.strip_prefix("  full log: "))
        .unwrap();
    assert!(std::path::Path::new(log).starts_with(s.logs()), "{log}");

    let out = s.jouet(&["commit", "a.txt", "-m", "change", "--json"], &[]);
    let doc = json(&out);
    assert_eq!(doc["failure"]["exit_code"], 1);
    assert_eq!(doc["failure"]["output"][0], "lint: a.txt is wrong");
    assert!(doc.get("committed").is_none());
    assert_eq!(s.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn review_package_writes_one_file_or_refuses() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    s.commit("a.txt", "a2\n", "change a");
    let out_file = s.dir.path().join("review.md");

    let out = s.jouet(
        &[
            "review-package",
            "HEAD~1",
            "HEAD",
            "-o",
            out_file.to_str().unwrap(),
            "--json",
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["commits"], 1);
    assert_eq!(doc["base"]["name"], "HEAD~1");
    let text = std::fs::read_to_string(&out_file).unwrap();
    assert!(text.contains("## Diff\n\n```diff\n"), "{text}");

    let out = s.jouet(&["review-package", "HEAD", "HEAD~1", "--json"], &[]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["error"]["code"], "precondition");
    let out = s.jouet(&["review-package", "HEAD"], &[]);
    assert_eq!(out.status.code(), Some(4));
    assert!(stderr(&out).contains("is empty"), "{}", stderr(&out));
    let out = s.jouet(&["review-package", "nope"], &[]);
    assert_eq!(out.status.code(), Some(3));

    // The default path is in the git directory, out of the working tree.
    let out = s.jouet(&["review-package", "HEAD~1"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("/.git/jouet/review-"),
        "{}",
        stdout(&out)
    );
    assert_eq!(s.git(&["status", "--porcelain"]), "");
}

#[test]
fn handoff_and_pickup_round_trip() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    s.write("a.txt", "a2\n");
    s.write("notes.txt", "n\n");
    let session = s.dir.path().join("home/.local/state/jouet/sessions");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("s1.json"),
        r#"{"runs":{"test":{"command":"cargo test","exit_code":0}}}"#,
    )
    .unwrap();

    let out = s.jouet(&["handoff", "--no-pr"], &[("JOUET_SESSION_ID", "s1")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("# Handoff: main\n"), "{text}");
    assert!(
        text.contains("  - test passed (exit 0): `cargo test`"),
        "{text}"
    );
    assert!(text.contains("wrote "), "{text}");
    assert!(
        !text.contains("jouet-handoff"),
        "the data stays in the file: {text}"
    );

    let out = s.jouet(&["pickup", "--json"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["accurate"], true);
    assert_eq!(doc["handoff"]["branch"], "main");
    assert_eq!(doc["handoff"]["changes"][0]["path"], "a.txt");
    assert_eq!(doc["handoff"]["untracked"][0], "notes.txt");

    s.git(&["commit", "-q", "-m", "more", "--", "a.txt"]);
    let out = s.jouet(&["pickup"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("## Drift since "), "{text}");
    assert!(
        text.contains("- `head`: 1 new commit since the handoff"),
        "{text}"
    );
    assert!(
        text.contains("- `uncommitted`: a.txt no longer has changes"),
        "{text}"
    );

    let file = s.dir.path().join("handoff.json");
    let out = s.jouet(&["handoff", "--no-pr", "-o", file.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = s.jouet(&["pickup", file.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

#[test]
fn pickup_without_a_handoff_is_not_found() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    let out = s.jouet(&["pickup", "--json"], &[]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["error"]["code"], "not_found");
    s.write("notes.md", "# not a handoff\n");
    let out = s.jouet(&["pickup", "notes.md"], &[]);
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn handoff_reads_the_pr_from_github() {
    let s = Sandbox::new();
    s.commit("a.txt", "a\n", "init");
    s.git(&["checkout", "-q", "-b", "feat/x"]);
    s.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/jdx/jouet.git",
    ]);
    let pr = serde_json::json!({
        "id": "PR_1", "number": 20, "title": "feat(git): add recipes",
        "url": "https://github.com/jdx/jouet/pull/20", "state": "OPEN", "isDraft": false,
        "baseRefName": "main", "headRefName": "feat/x", "headRefOid": "abc",
        "repository": { "nameWithOwner": "jdx/jouet" },
        "mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY", "reviewDecision": null,
        "autoMergeRequest": null,
        "reviewThreads": { "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": [] },
        "timelineItems": { "nodes": [] },
        "commits": { "nodes": [] }
    });
    let body = serde_json::json!({
        "data": { "l0": { "open": { "nodes": [pr] }, "newest": { "nodes": [pr] } } }
    })
    .to_string();
    let fake = common::serve(move |_, _| (200, body.clone()));

    let out = s.jouet(
        &["handoff", "-o", "-", "--json"],
        &[
            ("JOUET_GITHUB_API_URL", fake.url.as_str()),
            ("JOUET_GITHUB_TOKEN", "t"),
        ],
    );

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert!(doc.get("path").is_none());
    assert_eq!(doc["handoff"]["pr"]["number"], 20);
    assert_eq!(doc["handoff"]["pr"]["attention"][0]["code"], "conflict");
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/graphql"));
    assert_eq!(r.token, "t");
    assert!(common::query(r).contains("pullRequests(headRefName"));
    assert_eq!(r.body["variables"]["b0"], "feat/x");
}
