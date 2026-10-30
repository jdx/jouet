use super::*;
use crate::testing::Repo;

fn opts() -> Options {
    Options::default()
}

/// A repository with an upstream one commit ahead of the remote, a staged,
/// an unstaged and a renamed file, and an untracked one.
fn busy_repo() -> (Repo, Repo) {
    let remote = Repo::new();
    remote.commit(
        &[("a.txt", "a\n"), ("b.txt", "b\n"), ("old name.txt", "o\n")],
        "init",
    );
    let repo = Repo::new();
    repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["checkout", "-q", "-b", "feat/x", "--track", "origin/main"]);
    repo.commit(&[("a.txt", "a2\n")], "feat: change a");
    repo.write("b.txt", "b2\n");
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    repo.write("staged.txt", "s\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("notes/todo.txt", "t\n");
    (remote, repo)
}

#[test]
fn collects_the_git_facts() {
    let (_remote, repo) = busy_repo();
    let h = collect(&repo.git_handle(), &opts()).unwrap();

    assert_eq!(h.branch.as_deref(), Some("feat/x"));
    assert_eq!(h.upstream.as_deref(), Some("origin/main"));
    assert_eq!((h.ahead, h.behind), (Some(1), Some(0)));
    assert_eq!(h.head, Some(repo.git(&["rev-parse", "HEAD"])));
    assert_eq!(h.head_subject.as_deref(), Some("feat: change a"));
    assert_eq!(h.operation, None);
    assert_eq!(
        h.changes,
        [
            Change {
                status: " M".into(),
                path: "b.txt".into(),
                from: None
            },
            Change {
                status: "R ".into(),
                path: "new name.txt".into(),
                from: Some("old name.txt".into())
            },
            Change {
                status: "A ".into(),
                path: "staged.txt".into(),
                from: None
            },
        ]
    );
    assert_eq!(h.untracked, ["notes/todo.txt"]);
    assert!(h.diff_sha256.is_some());
    assert_eq!(
        std::path::Path::new(&h.worktree).canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );
    assert!(h.notes[0].contains("no session ID"), "{:?}", h.notes);
}

#[test]
fn markdown_round_trips_through_parse() {
    let (_remote, repo) = busy_repo();
    let mut h = collect(&repo.git_handle(), &opts()).unwrap();
    h.pr = Some(Pr {
        repo: "jdx/jouet".into(),
        number: 20,
        title: "feat(git): add recipes --> soon".into(),
        url: "https://github.com/jdx/jouet/pull/20".into(),
        state: "open".into(),
        attention: vec![Attention {
            code: "checks_failing".into(),
            message: "1 failing: lint".into(),
        }],
    });
    h.checks = vec![CheckRun {
        kind: "test".into(),
        command: "cargo test".into(),
        exit_code: 101,
    }];

    let text = h.markdown(true);
    assert!(text.starts_with("# Handoff: feat/x\n"), "{text}");
    assert!(
        text.contains("- **Branch:** feat/x, tracking origin/main (1 ahead, 0 behind)"),
        "{text}"
    );
    assert!(
        text.contains("  - `checks_failing`: 1 failing: lint"),
        "{text}"
    );
    assert!(
        text.contains("  - test failed (exit 101): `cargo test`"),
        "{text}"
    );
    assert!(
        text.contains("  - `R ` new name.txt (from old name.txt)"),
        "{text}"
    );
    assert!(
        text.contains("- **Untracked (1):**\n  - notes/todo.txt"),
        "{text}"
    );
    // The title's `-->` does not end the data comment early.
    let data = &text[text.find("<!-- jouet-handoff").unwrap()..];
    assert_eq!(data.matches("-->").count(), 1, "{text}");
    assert_eq!(parse(&text).unwrap(), h);
    assert!(!h.markdown(false).contains("jouet-handoff"));

    let json = serde_json::to_string(&h).unwrap();
    assert_eq!(parse(&json).unwrap(), h);
    assert!(
        parse("# just notes\n")
            .unwrap_err()
            .contains("not a jouet handoff")
    );
}

#[test]
fn long_lists_say_how_much_is_left_out() {
    let mut h = Handoff {
        untracked: (0..60).map(|i| format!("f{i}")).collect(),
        ..Handoff::default()
    };
    h.created_at = "2026-10-30T12:00:00Z".into();
    let text = h.markdown(true);
    assert!(text.contains("- **Untracked (60):**"), "{text}");
    assert!(
        text.contains("  - f49\n  - ... 10 more (the data at the end"),
        "{text}"
    );
    assert!(!text.contains("  - f50\n"), "{text}");
    assert_eq!(parse(&text).unwrap().untracked.len(), 60);
}

#[test]
fn pickup_finds_nothing_when_nothing_moved() {
    let (_remote, repo) = busy_repo();
    let git = repo.git_handle();
    let h = collect(&git, &opts()).unwrap();
    assert_eq!(drift(&git, &h).unwrap(), []);
}

#[test]
fn pickup_reports_each_kind_of_drift() {
    let (_remote, repo) = busy_repo();
    let git = repo.git_handle();
    let h = collect(&git, &opts()).unwrap();

    repo.write("b.txt", "b3\n");
    let codes = |d: Vec<Drift>| d.iter().map(|d| d.code).collect::<Vec<_>>();
    let d = drift(&git, &h).unwrap();
    assert_eq!(codes(d.clone()), ["uncommitted"]);
    assert!(d[0].message.contains("changes differ"), "{d:?}");

    repo.write("scratch.txt", "x\n");
    repo.git(&["add", "b.txt"]);
    let d = drift(&git, &h).unwrap();
    assert_eq!(codes(d.clone()), ["uncommitted", "untracked"]);
    assert!(d[0].message.contains("b.txt was ` M`, now `M `"), "{d:?}");
    assert!(d[1].message.contains("now untracked: scratch.txt"), "{d:?}");

    repo.git(&["commit", "-q", "-m", "more", "--", "b.txt"]);
    let d = drift(&git, &h).unwrap();
    assert!(
        d[0].message.contains("1 new commit since the handoff"),
        "{d:?}"
    );

    repo.git(&["reset", "-q", "--soft", "HEAD~2"]);
    let d = drift(&git, &h).unwrap();
    assert!(d[0].message.contains("moved back 1 commit"), "{d:?}");
    assert!(
        d.iter()
            .any(|d| d.code == "behind" || d.code == "uncommitted")
    );

    repo.git(&["switch", "-q", "-c", "other"]);
    let d = drift(&git, &h).unwrap();
    assert_eq!(d[0].code, "branch");
    assert!(d[0].message.contains("was feat/x, now other"), "{d:?}");
}

/// Stages `staged` for `path`, then puts `worktree` back in the working
/// tree, so only the index differs from what was there before.
fn restage(repo: &Repo, path: &str, staged: &str, worktree: &str) {
    repo.write(path, staged);
    repo.git(&["add", "--", path]);
    repo.write(path, worktree);
}

#[test]
fn pickup_notices_staged_changes_when_the_working_tree_is_unchanged() {
    let repo = Repo::new();
    let git = repo.git_handle();
    repo.commit(&[("a.txt", "one\n")], "init");
    restage(&repo, "a.txt", "staged\n", "worktree\n");
    let h = collect(&git, &opts()).unwrap();
    assert_eq!(h.changes[0].status, "MM");

    // Another session stages different contents; the working tree, and so
    // `git diff HEAD`, and the status letters are what they were.
    restage(&repo, "a.txt", "restaged\n", "worktree\n");
    let now = collect(&git, &opts()).unwrap();
    assert_eq!(now.changes, h.changes);

    let d = drift(&git, &h).unwrap();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].code, "uncommitted");
    assert!(
        d[0].message.contains("staged or unstaged changes differ"),
        "{d:?}"
    );
}

#[test]
fn pickup_notices_staged_changes_on_an_unborn_branch() {
    let repo = Repo::new();
    let git = repo.git_handle();
    restage(&repo, "a.txt", "staged\n", "worktree\n");
    let h = collect(&git, &opts()).unwrap();
    assert_eq!(h.head, None);
    assert!(h.diff_sha256.is_some());
    assert_eq!(drift(&git, &h).unwrap(), []);

    restage(&repo, "a.txt", "restaged\n", "worktree\n");
    let d = drift(&git, &h).unwrap();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].code, "uncommitted");
}

#[test]
fn pickup_notices_a_rewritten_head_and_an_operation() {
    let repo = Repo::new();
    let git = repo.git_handle();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.commit(&[("a.txt", "b\n")], "two");
    let h = collect(&git, &opts()).unwrap();
    repo.git(&["commit", "-q", "--amend", "-m", "two, reworded"]);
    let d = drift(&git, &h).unwrap();
    assert!(d[0].message.contains("rewritten"), "{d:?}");

    repo.git(&["checkout", "-q", "-b", "side", "HEAD~1"]);
    repo.commit(&[("a.txt", "side\n")], "side");
    let h = collect(&git, &opts()).unwrap();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["merge", "-q", "main"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge should conflict");
    let now = collect(&git, &opts()).unwrap();
    assert_eq!(now.operation.as_deref(), Some("merge"));
    assert_eq!(now.changes[0].status, "UU");
    let d = drift(&git, &h).unwrap();
    assert!(d.iter().any(|d| d.code == "operation"), "{d:?}");
}

#[test]
fn the_unborn_branch_has_no_head() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let h = collect(&repo.git_handle(), &opts()).unwrap();
    assert_eq!(h.head, None);
    assert_eq!(h.branch.as_deref(), Some("main"));
    assert_eq!(h.untracked, ["a.txt"]);
    assert!(h.markdown(true).contains("none yet (unborn branch)"));
}

#[test]
fn checks_come_from_the_session_file() {
    let repo = Repo::new();
    repo.commit(&[("a.txt", "a\n")], "init");
    let sessions = tempfile::tempdir().unwrap();
    std::fs::write(
        sessions.path().join("s1.json"),
        r#"{"runs":{"lint":{"command":"mise run lint","exit_code":0}}}"#,
    )
    .unwrap();
    let mut o = opts();
    o.session = Some((sessions.path().to_path_buf(), "s1".into()));
    let h = collect(&repo.git_handle(), &o).unwrap();
    assert_eq!(h.checks.len(), 1);
    assert!(h.notes.is_empty(), "{:?}", h.notes);

    o.session = Some((sessions.path().to_path_buf(), "s2".into()));
    let h = collect(&repo.git_handle(), &o).unwrap();
    assert!(h.checks.is_empty());
    assert!(
        h.notes[0].contains("none recorded for session s2"),
        "{:?}",
        h.notes
    );
}

#[test]
fn writes_markdown_or_json_by_extension() {
    let (_remote, repo) = busy_repo();
    let h = collect(&repo.git_handle(), &opts()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("deep/handoff.md");
    write(&md, &h).unwrap();
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(text.starts_with("# Handoff"));
    assert_eq!(parse(&text).unwrap(), h);
    let json = dir.path().join("handoff.json");
    write(&json, &h).unwrap();
    let text = std::fs::read_to_string(&json).unwrap();
    assert!(text.starts_with('{'));
    assert_eq!(parse(&text).unwrap(), h);
    assert!(
        default_path(&repo.git_handle())
            .unwrap()
            .ends_with(".git/jouet/handoff.md")
    );
}
