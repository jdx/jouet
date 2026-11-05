
// Appended by the benchmark's checker: upstream's tests from jdx/pitchfork#737
// (0a5e3b3 and 8fa212b), run with a symlinked TMPDIR so that a temp path is not canonical,
// as on the Windows runner. The symlink test compares against the canonical
// path; upstream's compared against `repo`, which only holds when the temp dir
// is canonical.
#[cfg(test)]
mod bench_hidden {
    use super::*;

    #[test]
    fn bench_find_project_root_finds_git_marker() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("my-repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let sub = repo.join("sub/dir");
        std::fs::create_dir_all(&sub).unwrap();

        assert_eq!(find_project_root(&sub), Some(repo.canonicalize().unwrap()));
    }

    #[test]
    fn bench_find_project_root_accepts_git_file_marker() {
        let temp = tempfile::tempdir().unwrap();
        let wt = temp.path().join("my-worktree");
        std::fs::create_dir(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /tmp/some-common-gitdir\n").unwrap();

        assert_eq!(find_project_root(&wt), Some(wt.canonicalize().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn bench_find_project_root_resolves_symlinked_start_dir() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("real-repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let sub = repo.join("sub/dir");
        std::fs::create_dir_all(&sub).unwrap();
        let link = temp.path().join("link-to-sub");
        symlink(&sub, &link).unwrap();

        assert_eq!(find_project_root(&link), Some(repo.canonicalize().unwrap()));
    }
}
