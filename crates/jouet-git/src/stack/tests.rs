use super::*;
use crate::testing::Repo;

/// main ← a ← b, with `lib.txt` on main that layer `a` fixes, and `b`
/// changing a file of its own and the end of `lib.txt`.
fn stacked() -> Repo {
    let repo = Repo::new();
    repo.commit(&[("lib.txt", "one\nbug\nthree\nfour\nfive\n")], "init");
    repo.git(&["checkout", "-q", "-b", "a"]);
    repo.commit(&[("a.txt", "a\n")], "a: feature");
    repo.git(&["checkout", "-q", "-b", "b"]);
    repo.commit(&[("b.txt", "b\n")], "b: feature");
    repo
}

fn stack() -> Stack {
    Stack {
        trunk: "main".into(),
        remote: "origin".into(),
        layers: vec![
            Layer {
                branch: "a".into(),
                parent: "main".into(),
                merged: false,
                parent_merged: false,
            },
            Layer {
                branch: "b".into(),
                parent: "a".into(),
                merged: false,
                parent_merged: false,
            },
        ],
    }
}

#[test]
fn a_stack_in_order_needs_nothing() {
    let repo = stacked();
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!(states.len(), 2);
    for s in &states {
        assert_eq!(s.contains_parent, Some(true), "{s:?}");
        assert!(s.needs().is_empty(), "{s:?}");
        // Never pushed: everything past the parent is unpushed.
        assert_eq!(s.unpushed, 1);
    }
}

#[test]
fn a_fix_to_the_parent_is_missing_until_merged() {
    let repo = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\nfour\nfive\n")], "a: fix");
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!(states[1].contains_parent, Some(false));
    assert_eq!(states[1].needs(), ["needs_parent"]);
    // Merged, with the child's own change to the same file, it is in order.
    repo.git(&["checkout", "-q", "b"]);
    repo.commit(&[("lib.txt", "one\nbug\nthree\nfour\nFIVE\n")], "b: tweak");
    repo.git(&["merge", "-q", "--no-edit", "a"]);
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert!(states[1].needs().is_empty(), "{:?}", states[1]);
}

#[test]
fn a_restack_that_reverts_the_parents_fix_is_flagged_by_content() {
    let repo = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\nfour\nfive\n")], "a: fix");
    repo.git(&["checkout", "-q", "b"]);
    // The merge records `a` as a parent but keeps b's tree: by ancestry the
    // restack looks done.
    repo.git(&["merge", "-q", "--no-edit", "-s", "ours", "a"]);
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!(states[1].contains_parent, Some(true));
    let fix = repo.git(&["log", "-1", "--format=%h a: fix", "a"]);
    assert_eq!(
        states[1].content,
        [FileProblem {
            path: "lib.txt".into(),
            problem: Problem::Reverted,
            commit: Some(fix),
        }]
    );
    assert_eq!(states[1].needs(), ["parent_changes_missing"]);

    // Part of the fix, kept differently, is missing rather than reverted.
    repo.commit(
        &[("lib.txt", "one\nbug\nthree\nfour\nfive\nsix\n")],
        "b: more",
    );
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!(states[1].content[0].problem, Problem::Missing);
    repo.commit(
        &[("lib.txt", "one\nfixed!\nthree\nfour\nfive\n")],
        "b: edit",
    );
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!(states[1].content[0].problem, Problem::Conflicting);
}

#[test]
fn the_childs_own_later_edit_of_the_parents_lines_is_not_flagged() {
    // `a` changes a line, and `b`, built on it, rewrites that line again.
    let repo = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\nfour\nfive\n")], "a: fix");
    repo.git(&["checkout", "-q", "b"]);
    repo.git(&["rebase", "-q", "a"]);
    repo.commit(
        &[("lib.txt", "one\nfixed better\nthree\nfour\nfive\n")],
        "b: rewrite the fix",
    );
    repo.commit(&[("b.txt", "b\nb\n")], "b: more");
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!(states[1].contains_parent, Some(true));
    assert!(states[1].needs().is_empty(), "{:?}", states[1]);

    // The same, after `a` is merged in rather than rebased onto.
    let repo = stacked();
    repo.git(&["checkout", "-q", "a"]);
    repo.commit(&[("lib.txt", "one\nfixed\nthree\nfour\nfive\n")], "a: fix");
    repo.git(&["checkout", "-q", "b"]);
    repo.git(&["merge", "-q", "--no-edit", "a"]);
    repo.commit(
        &[("lib.txt", "one\nthree\nfour\nfive\n")],
        "b: drop the line",
    );
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert!(states[1].needs().is_empty(), "{:?}", states[1]);
}

#[test]
fn unpushed_and_behind_come_from_the_remote_tracking_branch() {
    let repo = stacked();
    let head = repo.git(&["rev-parse", "b"]);
    repo.git(&["update-ref", "refs/remotes/origin/b", &head]);
    repo.git(&["update-ref", "refs/remotes/origin/a", "a"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    repo.commit(&[("b.txt", "b2\n")], "b: unpushed");
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!((states[1].unpushed, states[1].behind), (1, 0));
    // The remote moved on without the local branch.
    repo.git(&["checkout", "-q", "-b", "elsewhere", &head]);
    let ahead = repo.commit(&[("c.txt", "c\n")], "pushed from elsewhere");
    repo.git(&["update-ref", "refs/remotes/origin/b", &ahead]);
    let states = inspect(&repo.git_handle(), &stack()).unwrap();
    assert_eq!((states[1].unpushed, states[1].behind), (1, 1));
    assert_eq!(states[1].needs(), ["behind_remote"]);
}

#[test]
fn after_the_parent_merges_the_child_needs_the_trunk() {
    let repo = stacked();
    // `a` is squash-merged into main.
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "-q", "--squash", "a"]);
    repo.git(&["commit", "-q", "-m", "a (#1)"]);
    let mut stack = stack();
    stack.layers[0].merged = true;
    stack.layers[1].parent_merged = true;
    let states = inspect(&repo.git_handle(), &stack).unwrap();
    assert!(states[0].needs().is_empty());
    assert!(states[1].needs_trunk);
    assert_eq!(states[1].needs(), ["needs_trunk"]);
    repo.git(&["checkout", "-q", "b"]);
    repo.git(&["merge", "-q", "--no-edit", "main"]);
    let states = inspect(&repo.git_handle(), &stack).unwrap();
    assert!(states[1].needs().is_empty(), "{:?}", states[1]);
}

#[test]
fn gh_stack_metadata_names_the_stack_holding_a_branch() {
    let text = r#"{"schemaVersion":1,"stacks":[
        {"trunk":{"branch":"main"},"branches":[{"branch":"x"}]},
        {"trunk":{"branch":"main","head":"abc"},"branches":[
            {"branch":"a","pullRequest":{"number":1}},
            {"branch":"b"}]}]}"#;
    assert_eq!(
        parse_gh_stack(text, "b"),
        Some(GhStack {
            trunk: "main".into(),
            branches: vec![("a".into(), Some(1)), ("b".into(), None)],
        })
    );
    assert_eq!(parse_gh_stack(text, "nope"), None);
    let repo = stacked();
    std::fs::write(repo.path().join(".git/gh-stack"), text).unwrap();
    assert_eq!(gh_stack(&repo.git_handle(), "a").unwrap().trunk, "main");
}
