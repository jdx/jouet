use super::*;
use crate::testing::Repo;

fn request(paths: &[&str], message: &str, logs: &Path) -> Request {
    Request {
        paths: paths.iter().map(|p| (*p).to_string()).collect(),
        message: message.into(),
        conventional: Policy::Off,
        types: conventional::DEFAULT_TYPES
            .iter()
            .map(|t| (*t).to_string())
            .collect(),
        log_dir: logs.to_path_buf(),
        ..Request::default()
    }
}

fn names(report: &Report) -> Vec<(&str, &str)> {
    report
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.status))
        .collect()
}

#[test]
fn commits_only_the_named_paths() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n"), ("b.txt", "b\n")], "init");
    repo.write("a.txt", "a2\n");
    // Another session's staged work, and a cache nobody meant to commit.
    repo.write("b.txt", "b2\n");
    repo.git(&["add", "b.txt"]);
    repo.write("test/.cache/blob", "junk\n");
    repo.write("new.txt", "new\n");

    let report = run(
        &repo.git_handle(),
        &request(&["a.txt", "new.txt"], "change a", logs.path()),
    )
    .unwrap();

    assert!(report.committed, "{report:?}");
    assert_eq!(
        names(&report),
        [("a.txt", "modified"), ("new.txt", "added")]
    );
    assert_eq!(report.files[0].added, Some(1));
    assert_eq!(report.files[0].removed, Some(1));
    assert_eq!(report.newly_tracked, ["new.txt"]);
    assert_eq!(report.branch.as_deref(), Some("main"));
    assert_eq!(report.sha, Some(repo.git(&["rev-parse", "HEAD"])));
    assert_eq!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"]),
        "a.txt\nnew.txt"
    );
    // b.txt is still staged, and the cache is still untracked.
    assert_eq!(
        repo.git(&["status", "--porcelain", "--untracked-files=all"]),
        "M  b.txt\n?? test/.cache/blob"
    );
}

#[test]
fn a_directory_covers_its_changes_and_new_files() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("src/a.rs", "a\n"), ("other.rs", "o\n")], "init");
    repo.write("src/a.rs", "a2\n");
    repo.write("src/new.rs", "new\n");
    repo.write("other.rs", "o2\n");

    let report = run(
        &repo.git_handle(),
        &request(&["src"], "change src", logs.path()),
    )
    .unwrap();

    assert_eq!(
        names(&report),
        [("src/a.rs", "modified"), ("src/new.rs", "added")]
    );
    assert_eq!(repo.git(&["status", "--porcelain"]), "M other.rs");
}

#[test]
fn paths_are_read_from_the_directory_git_runs_in() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("sub/a.txt", "a\n")], "init");
    repo.write("sub/a.txt", "a2\n");
    repo.write("sub/new.txt", "n\n");

    let git = Git::new(repo.path().join("sub"));
    let report = run(&git, &request(&["a.txt", "new.txt"], "sub", logs.path())).unwrap();

    assert_eq!(
        names(&report),
        [("sub/a.txt", "modified"), ("sub/new.txt", "added")]
    );
    let err = run(&git, &request(&[".."], "all", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::Usage);
}

#[test]
fn deletions_are_committed() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("gone.txt", "x\n"), ("kept.txt", "k\n")], "init");
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();

    let report = run(
        &repo.git_handle(),
        &request(&["gone.txt"], "remove", logs.path()),
    )
    .unwrap();

    assert_eq!(names(&report), [("gone.txt", "deleted")]);
    assert_eq!(report.files[0].removed, Some(1));
}

#[test]
fn the_first_commit_works_on_an_unborn_branch() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");

    let report = run(
        &repo.git_handle(),
        &request(&["a.txt"], "first", logs.path()),
    )
    .unwrap();

    assert!(report.committed);
    assert_eq!(names(&report), [("a.txt", "added")]);
    assert_eq!(repo.git(&["status", "--porcelain"]), "?? b.txt");
    // The report says what is left, as `git status` would.
    assert_eq!(report.left.as_deref(), Some(&["b.txt".to_string()][..]));
    let report = run(
        &repo.git_handle(),
        &request(&["b.txt"], "second", logs.path()),
    )
    .unwrap();
    assert_eq!(report.left.as_deref(), Some(&[][..]));
}

#[test]
fn the_whole_tree_is_refused() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.write("a.txt", "a2\n");
    let git = repo.git_handle();
    let top = repo.path().display().to_string();
    for path in [".", "./", &top, "sub/.."] {
        let err = run(&git, &request(&[path], "all", logs.path())).unwrap_err();
        assert_eq!(err.exit, Exit::Usage, "{path}");
        assert!(err.message.contains("whole repository"), "{path}: {err:?}");
    }
    let err = run(&git, &request(&[], "none", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::Usage);
    let err = run(&git, &request(&["../x"], "outside", logs.path())).unwrap_err();
    assert!(err.message.contains("outside the repository"), "{err:?}");
    // A glob is a file name, not a pattern.
    let err = run(&git, &request(&["*.txt"], "glob", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::NotFound, "{err:?}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn a_path_without_changes_is_refused() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(
        &[
            ("a.txt", "a\n"),
            ("b.txt", "b\n"),
            (".gitignore", "*.log\n"),
        ],
        "init",
    );
    repo.write("a.txt", "a2\n");
    repo.write("debug.log", "x\n");
    let git = repo.git_handle();

    let err = run(&git, &request(&["a.txt", "b.txt"], "m", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::Precondition);
    assert!(err.message.contains("`b.txt` has no changes"), "{err:?}");

    let err = run(&git, &request(&["missing.txt"], "m", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::NotFound);

    let err = run(&git, &request(&["debug.log"], "m", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::Precondition);
    assert!(err.message.contains("ignored"), "{err:?}");

    let err = run(&git, &request(&["a.txt"], "  \n", logs.path())).unwrap_err();
    assert_eq!(err.exit, Exit::Usage);
    // Nothing was staged or committed along the way.
    assert_eq!(repo.git(&["status", "--porcelain"]), "M a.txt");
}

#[test]
fn trailers_are_appended_once() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.write("a.txt", "a2\n");
    let mut req = request(
        &["a.txt"],
        "feat: change a\n\nWhy it changed.\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n",
        logs.path(),
    );
    req.trailers = vec![
        "Co-Authored-By: Claude <noreply@anthropic.com>".into(),
        "AI-Assisted: Tool: Claude Code; model: unavailable; version: 2.1.309".into(),
    ];

    let report = run(&repo.git_handle(), &req).unwrap();

    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]),
        "feat: change a\n\nWhy it changed.\n\n\
         Co-Authored-By: Claude <noreply@anthropic.com>\n\
         AI-Assisted: Tool: Claude Code; model: unavailable; version: 2.1.309"
    );
    assert_eq!(report.subject, "feat: change a");
    assert!(report.message.ends_with("version: 2.1.309"), "{report:?}");

    req.trailers = vec!["not a trailer".into()];
    repo.write("a.txt", "a3\n");
    let err = run(&repo.git_handle(), &req).unwrap_err();
    assert_eq!(err.exit, Exit::Usage);
}

#[test]
fn a_dry_run_commits_nothing() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.write("a.txt", "a2\n");
    repo.write("new.txt", "n\n");
    let mut req = request(&["a.txt", "new.txt"], "change", logs.path());
    req.dry_run = true;
    req.trailers = vec!["Co-Authored-By: X <x@example.com>".into()];

    let report = run(&repo.git_handle(), &req).unwrap();

    assert!(report.dry_run && !report.committed);
    assert_eq!(
        names(&report),
        [("a.txt", "modified"), ("new.txt", "added")]
    );
    assert_eq!(
        report.message,
        "change\n\nCo-Authored-By: X <x@example.com>\n"
    );
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(repo.git(&["status", "--porcelain"]), "M a.txt\n?? new.txt");
}

#[test]
fn conventional_subjects_are_checked_where_the_repo_asks() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.write("a.txt", "a2\n");
    let git = repo.git_handle();
    let mut req = request(&["a.txt"], "Change a", logs.path());
    req.conventional = Policy::Refuse;
    req.dry_run = true;
    // No workflow or config: nothing to check against.
    assert!(run(&git, &req).unwrap().warnings.is_empty());

    repo.commit(
        &[(
            ".github/workflows/conventional-commits.yml",
            "name: conventional-commits\n",
        )],
        "ci",
    );
    let err = run(&git, &req).unwrap_err();
    assert_eq!(err.exit, Exit::Precondition);
    assert!(
        err.message
            .contains(".github/workflows/conventional-commits.yml"),
        "{err:?}"
    );

    req.conventional = Policy::Warn;
    let report = run(&git, &req).unwrap();
    assert_eq!(report.warnings.len(), 1, "{report:?}");

    req.conventional = Policy::Off;
    assert!(run(&git, &req).unwrap().warnings.is_empty());

    req.conventional = Policy::Refuse;
    req.message = "fix: change a".into();
    assert!(run(&git, &req).unwrap().warnings.is_empty());
}

#[cfg(unix)]
#[test]
fn a_failing_hook_is_reported_and_nothing_is_left_staged() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    let mut script = String::from("#!/bin/sh\n");
    for i in 1..=50 {
        script.push_str(&format!("echo \"lint line {i}\"\n"));
    }
    script.push_str("echo 'a.txt: trailing whitespace' >&2\nexit 1\n");
    repo.hook("pre-commit", &script);
    repo.write("a.txt", "a2 \n");
    repo.write("new.txt", "n\n");

    let report = run(
        &repo.git_handle(),
        &request(&["a.txt", "new.txt"], "change", logs.path()),
    )
    .unwrap();

    assert!(!report.committed);
    let failure = report.failure.expect("a failure");
    assert_eq!(failure.exit_code, Some(1));
    assert_eq!(failure.output.len(), HOOK_TAIL);
    assert_eq!(
        failure.output.last().map(String::as_str),
        Some("a.txt: trailing whitespace")
    );
    assert_eq!(failure.lines_omitted, 11);
    let log = std::fs::read_to_string(&failure.log_path).unwrap();
    assert!(log.contains("lint line 1\n"), "{log}");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(repo.git(&["status", "--porcelain"]), "M a.txt\n?? new.txt");
}

#[cfg(unix)]
#[test]
fn hooks_run_and_a_commit_msg_hook_can_rewrite_the_message() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.hook(
        "commit-msg",
        "#!/bin/sh\nprintf '\\nReviewed-By: hook\\n' >> \"$1\"\n",
    );
    repo.write("a.txt", "a2\n");

    let report = run(
        &repo.git_handle(),
        &request(&["a.txt"], "change", logs.path()),
    )
    .unwrap();

    assert!(report.committed);
    assert_eq!(report.message, "change\n\nReviewed-By: hook");
    // A commit that succeeded leaves no log behind.
    assert_eq!(std::fs::read_dir(logs.path()).unwrap().count(), 0);
}

/// Reports every file in the scanned index that differs from HEAD and
/// contains `hunter2-secret`, as a secret named `PASSWORD`.
struct Grep {
    seen: std::cell::RefCell<Vec<String>>,
}

impl Scanner for Grep {
    fn scan(&self, top: &Path, index: &Path) -> Result<SecretScan, Error> {
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(top)
                .args(args)
                .env("GIT_INDEX_FILE", index)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let names = git(&["diff", "--cached", "--name-only", "--diff-filter=ACMR"]);
        let mut findings = Vec::new();
        for name in names.lines() {
            self.seen.borrow_mut().push(name.to_string());
            if git(&["show", &format!(":{name}")]).contains("hunter2-secret") {
                findings.push(SecretFinding {
                    secret: "PASSWORD".into(),
                    path: name.into(),
                    line: 1,
                    ..SecretFinding::default()
                });
            }
        }
        Ok(SecretScan {
            tool: "grep".into(),
            status: if findings.is_empty() {
                ScanStatus::Clean
            } else {
                ScanStatus::Found
            },
            files_scanned: Some(self.seen.borrow().len() as u64),
            findings,
            reason: None,
            notes: Vec::new(),
        })
    }
}

#[test]
fn the_scanner_sees_exactly_what_would_be_committed() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n"), ("b.txt", "b\n")], "init");
    repo.write("a.txt", "a2\n");
    // Staged by someone else, with a secret: not part of this commit.
    repo.write("b.txt", "hunter2-secret\n");
    repo.git(&["add", "b.txt"]);
    repo.write("dir/new.txt", "new\n");
    let grep = Grep {
        seen: std::cell::RefCell::default(),
    };

    let report = run_scanned(
        &repo.git_handle(),
        &request(&["a.txt", "dir"], "change a", logs.path()),
        Some(&grep),
    )
    .unwrap();

    assert!(report.committed, "{report:?}");
    assert_eq!(*grep.seen.borrow(), ["a.txt", "dir/new.txt"]);
    let scan = report.secret_scan.unwrap();
    assert_eq!(scan.status, ScanStatus::Clean);
    // The other session's staged file is still staged.
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "b.txt");
}

#[test]
fn a_secret_refuses_the_commit_and_leaves_the_checkout_alone() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    let head = repo.commit(&[("a.txt", "a\n")], "init");
    repo.write("a.txt", "a2\n");
    repo.write("config/app.env", "PASSWORD=hunter2-secret\n");
    let grep = Grep {
        seen: std::cell::RefCell::default(),
    };

    let report = run_scanned(
        &repo.git_handle(),
        &request(&["a.txt", "config"], "add config", logs.path()),
        Some(&grep),
    )
    .unwrap();

    assert!(!report.committed);
    assert!(report.secret_found());
    let scan = report.secret_scan.unwrap();
    assert_eq!(scan.findings[0].path, "config/app.env");
    assert_eq!(scan.findings[0].secret, "PASSWORD");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    // Nothing staged, and the new file is still untracked.
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(
        repo.git(&["status", "--porcelain", "--untracked-files=all"]),
        "M a.txt\n?? config/app.env"
    );
}

#[test]
fn a_dry_run_does_not_scan() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.write("a.txt", "hunter2-secret\n");
    let grep = Grep {
        seen: std::cell::RefCell::default(),
    };
    let mut req = request(&["a.txt"], "change a", logs.path());
    req.dry_run = true;

    let report = run_scanned(&repo.git_handle(), &req, Some(&grep)).unwrap();

    assert!(report.secret_scan.is_none());
    assert!(grep.seen.borrow().is_empty());
}

#[test]
fn an_unborn_branch_is_scanned_too() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.write("a.txt", "hunter2-secret\n");
    let grep = Grep {
        seen: std::cell::RefCell::default(),
    };

    let report = run_scanned(
        &repo.git_handle(),
        &request(&["a.txt"], "first", logs.path()),
        Some(&grep),
    )
    .unwrap();

    assert!(report.secret_found(), "{report:?}");
    assert!(repo.git_handle().head().is_none());
}

/// hk and other fixers rewrite files in pre-commit and stage the fix. With
/// `--only`, git commits the fix but writes the paths into the real index
/// as they were before the hook, so the work tree and the index disagreed
/// with HEAD, and the report said they were still uncommitted.
#[cfg(unix)]
#[test]
fn a_fix_a_hook_stages_leaves_the_index_at_the_commit() {
    let repo = Repo::new();
    let logs = tempfile::tempdir().unwrap();
    repo.commit(&[("a.txt", "a\n")], "init");
    repo.hook(
        "pre-commit",
        "#!/bin/sh\nprintf 'a2\\n' > a.txt\ngit add a.txt\n",
    );
    repo.write("a.txt", "a2 \n");

    let report = run(
        &repo.git_handle(),
        &request(&["a.txt"], "change", logs.path()),
    )
    .unwrap();

    assert!(report.committed, "{report:?}");
    assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "a2");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert_eq!(report.left, Some(Vec::new()));
}
