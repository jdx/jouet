use super::super::{Layer, Stack, inspect};
use super::*;
use crate::testing::Repo;

/// main ← a ← b ← c, all pushed to a bare `origin`, with `main` checked
/// out so every layer is merged in a temporary worktree.
fn stacked() -> (Repo, tempfile::TempDir) {
    let origin = tempfile::tempdir().unwrap();
    let repo = Repo::new();
    std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(origin.path())
        .output()
        .unwrap();
    repo.git(&["remote", "add", "origin", &origin.path().to_string_lossy()]);
    repo.commit(&[("lib.txt", "one\nbug\nthree\n")], "init");
    repo.git(&["checkout", "-q", "-b", "a"]);
    repo.commit(&[("a.txt", "a\n")], "a");
    repo.git(&["checkout", "-q", "-b", "b"]);
    repo.commit(&[("b.txt", "b\n")], "b");
    repo.git(&["checkout", "-q", "-b", "c"]);
    repo.commit(&[("c.txt", "c\n")], "c");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["push", "-q", "origin", "main", "a", "b", "c"]);
    repo.git(&["fetch", "-q", "origin"]);
    (repo, origin)
}

fn stack() -> Stack {
    let layer = |branch: &str, parent: &str| Layer {
        branch: branch.into(),
        parent: parent.into(),
        merged: false,
        parent_merged: false,
    };
    Stack {
        trunk: "main".into(),
        remote: "origin".into(),
        layers: vec![layer("a", "main"), layer("b", "a"), layer("c", "b")],
    }
}

const RUN: Options = Options {
    dry_run: false,
    push: false,
    fetch: true,
};

fn actions(report: &Report) -> Vec<(&str, &str)> {
    report
        .steps
        .iter()
        .map(|s| (s.branch.as_str(), s.action))
        .collect()
}

fn worktrees_now(repo: &Repo) -> usize {
    repo.git(&["worktree", "list"]).lines().count()
}

#[test]
fn a_fix_on_the_bottom_layer_is_merged_up_the_stack_and_verified() {
    let (repo, _origin) = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\n")], "a: fix");
    repo.git(&["checkout", "-q", "main"]);
    let git = repo.git_handle();

    let plan = sync(
        &git,
        &stack(),
        Options {
            dry_run: true,
            ..RUN
        },
    );
    assert_eq!(
        actions(&plan),
        [
            ("a", "up_to_date"),
            ("b", "would_merge"),
            ("c", "would_merge")
        ]
    );
    let before = repo.git(&["rev-parse", "b"]);

    let report = sync(&git, &stack(), RUN);
    assert_eq!(report.stopped, None);
    assert_eq!(
        actions(&report),
        [("a", "up_to_date"), ("b", "merged"), ("c", "merged")]
    );
    assert_eq!(report.steps[1].from.as_deref(), Some("a"));
    assert_ne!(repo.git(&["rev-parse", "b"]), before);
    // The fix reached the top, the old commits stay, and nothing was left
    // behind.
    assert_eq!(repo.git(&["show", "c:lib.txt"]), "one\nfixed\nthree");
    assert!(
        repo.git(&["merge-base", "--is-ancestor", &before, "b"])
            .is_empty()
    );
    assert_eq!(worktrees_now(&repo), 1);
    assert!(
        inspect(&git, &stack())
            .unwrap()
            .iter()
            .all(|s| s.needs().is_empty())
    );
    // Nothing was pushed.
    assert_eq!(repo.git(&["rev-parse", "origin/b"]), before);
    // A second run has nothing to do.
    let again = sync(&git, &stack(), RUN);
    assert!(again.steps.iter().all(|s| s.action == "up_to_date"));
}

#[test]
fn the_branch_checked_out_here_is_merged_in_place_when_clean() {
    let (repo, _origin) = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\n")], "a: fix");
    repo.git(&["checkout", "-q", "b"]);
    repo.write("b.txt", "edited\n");
    let git = repo.git_handle();
    let report = sync(&git, &stack(), RUN);
    let stopped = report.stopped.as_ref().unwrap();
    assert_eq!((stopped.branch.as_str(), stopped.reason), ("b", "dirty"));
    assert_eq!(report.exit(), Exit::Precondition);

    repo.git(&["checkout", "--", "b.txt"]);
    let report = sync(&git, &stack(), RUN);
    assert_eq!(report.stopped, None);
    assert_eq!(repo.git(&["show", "HEAD:lib.txt"]), "one\nfixed\nthree");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn a_local_branch_behind_its_remote_is_refused() {
    let (repo, _origin) = stacked();
    // Someone else pushed to b; the local b does not have it.
    repo.git(&["checkout", "-q", "b"]);
    repo.commit(&[("b.txt", "theirs\n")], "pushed elsewhere");
    repo.git(&["push", "-q", "origin", "b"]);
    repo.git(&["reset", "-q", "--hard", "HEAD^"]);
    repo.git(&["checkout", "-q", "main"]);
    let report = sync(&repo.git_handle(), &stack(), RUN);
    let stopped = report.stopped.unwrap();
    assert_eq!(
        (stopped.branch.as_str(), stopped.reason),
        ("b", "behind_remote")
    );
    assert!(
        stopped.next.contains("`git merge origin/b`"),
        "{}",
        stopped.next
    );
}

#[test]
fn a_conflict_stops_there_and_leaves_the_branch_as_it_was() {
    let (repo, _origin) = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\n")], "a: fix");
    repo.git(&["checkout", "-q", "b"]);
    repo.commit(&[("lib.txt", "one\nb's way\nthree\n")], "b: other fix");
    repo.git(&["checkout", "-q", "main"]);
    let before = repo.git(&["rev-parse", "b"]);
    let report = sync(&repo.git_handle(), &stack(), RUN);
    let stopped = report.stopped.clone().unwrap();
    assert_eq!((stopped.branch.as_str(), stopped.reason), ("b", "conflict"));
    assert_eq!(stopped.files, ["lib.txt"]);
    assert_eq!(
        stopped.next,
        "`git switch b && git merge a`, resolve them and `git commit`, then rerun `jouet stack sync`"
    );
    assert_eq!(report.exit(), Exit::Failed);
    assert_eq!(repo.git(&["rev-parse", "b"]), before);
    assert_eq!(worktrees_now(&repo), 1);
    // The layers above were not touched.
    assert_eq!(actions(&report), [("a", "up_to_date")]);
}

#[test]
fn after_a_squash_merge_the_trunk_is_merged_in() {
    let (repo, _origin) = stacked();
    repo.git(&["merge", "-q", "--squash", "a"]);
    repo.git(&["commit", "-q", "-m", "a (#1)"]);
    repo.git(&["push", "-q", "origin", "main"]);
    let mut stack = stack();
    stack.layers[0].merged = true;
    stack.layers[1].parent_merged = true;
    let report = sync(&repo.git_handle(), &stack, RUN);
    assert_eq!(report.stopped, None);
    assert_eq!(
        actions(&report),
        [("a", "skipped_merged"), ("b", "merged"), ("c", "merged")]
    );
    assert_eq!(report.steps[1].from.as_deref(), Some("origin/main"));
    assert!(
        repo.git(&["merge-base", "--is-ancestor", "origin/main", "c"])
            .is_empty()
    );
}

#[test]
fn a_layer_that_already_reverted_its_parents_change_stops_sync() {
    let (repo, _origin) = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\n")], "a: fix");
    repo.git(&["checkout", "-q", "b"]);
    repo.git(&["merge", "-q", "--no-edit", "-s", "ours", "a"]);
    repo.git(&["checkout", "-q", "main"]);
    let report = sync(&repo.git_handle(), &stack(), RUN);
    let stopped = report.stopped.unwrap();
    assert_eq!((stopped.branch.as_str(), stopped.reason), ("b", "content"));
    assert_eq!(stopped.files, ["lib.txt"]);
    assert!(
        stopped.message.contains("lib.txt (undoes "),
        "{}",
        stopped.message
    );
    assert!(
        stopped.next.contains("`git checkout a -- lib.txt`"),
        "{}",
        stopped.next
    );
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_left_to_it() {
    let (repo, _origin) = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\n")], "a: fix");
    repo.git(&["checkout", "-q", "main"]);
    let other = tempfile::tempdir().unwrap();
    let wt = other.path().join("wt-b");
    repo.git(&["worktree", "add", "-q", &wt.to_string_lossy(), "b"]);
    let report = sync(&repo.git_handle(), &stack(), RUN);
    let stopped = report.stopped.unwrap();
    assert_eq!(
        (stopped.branch.as_str(), stopped.reason),
        ("b", "checked_out_elsewhere")
    );
    assert!(
        stopped.next.contains("run `jouet stack sync` in "),
        "{}",
        stopped.next
    );
}

#[test]
fn push_sends_the_layers_ahead_of_their_remote_and_never_forces() {
    let (repo, _origin) = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\n")], "a: fix");
    repo.git(&["checkout", "-q", "main"]);
    let report = sync(&repo.git_handle(), &stack(), Options { push: true, ..RUN });
    assert_eq!(report.stopped, None);
    let pushed: Vec<(&str, bool)> = report
        .pushed
        .iter()
        .map(|p| (p.branch.as_str(), p.ok))
        .collect();
    assert_eq!(pushed, [("a", true), ("b", true), ("c", true)]);
    repo.git(&["fetch", "-q", "origin"]);
    for branch in ["a", "b", "c"] {
        assert_eq!(
            repo.git(&["rev-parse", branch]),
            repo.git(&["rev-parse", &format!("origin/{branch}")])
        );
    }
}
