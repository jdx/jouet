
// Appended by the benchmark's checker: upstream's test from jdx/pitchfork#737
// (0fab67f), the one CI failed, run with a symlinked TMPDIR. The fast path's
// entry is the canonical checkout path, as `git worktree list` reports it.
#[cfg(test)]
mod bench_hidden {
    use super::*;

    #[test]
    fn bench_discover_git_worktrees_single_checkout_no_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir(&repo).unwrap();
        let git_dir = repo.join(".git");
        std::fs::create_dir_all(git_dir.join("refs")).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let entries = discover_git_worktrees(&repo);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, repo.canonicalize().unwrap());
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].sanitized_branch, "main");
    }
}
