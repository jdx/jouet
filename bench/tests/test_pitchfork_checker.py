import json
import os
import select
import signal
import subprocess
import tempfile
import unittest
from pathlib import Path

CHECKER = Path(__file__).resolve().parents[1] / "tasks/pitchfork-worktree-pr/check.py"


@unittest.skipUnless(os.name == "posix", "the task exercises Unix symlinks")
class PitchforkCheckerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "TMPDIR": str(self.root),
            "GIT_AUTHOR_NAME": "Test",
            "GIT_AUTHOR_EMAIL": "test@example.invalid",
            "GIT_COMMITTER_NAME": "Test",
            "GIT_COMMITTER_EMAIL": "test@example.invalid",
        }
        self.git("init", "-q")
        for name in ("src/pitchfork_toml.rs", "src/proxy/worktree.rs", "docs/concepts/namespaces.md"):
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("original\n")
        self.git("add", ".")
        self.git("commit", "-qm", "base")
        self.env["BENCH_HEAD_SHA"] = self.git("rev-parse", "HEAD")
        (self.repo / "docs/concepts/namespaces.md").write_text("explicit namespace overrides\n")
        self.git("add", "docs/concepts/namespaces.md")
        self.git("commit", "-qm", "solution")
        threads = []
        for thread_id in ("PRRT_kwDOdemo1", "PRRT_kwDOdemo2", "PRRT_kwDOdemo3"):
            threads.append({"id": thread_id, "isResolved": True, "comments": {"nodes": [
                {"author": {"login": "reviewer"}}, {"author": {"login": "bench"}},
            ]}})
        state = {"graphql": {"viewer": {"login": "bench"}, "repository": {"pullRequests": {"nodes": [
            {"number": 737, "reviewThreads": {"nodes": threads}},
        ]}}}}
        state_path = self.root / "state.json"
        state_path.write_text(json.dumps(state))
        self.env["BENCH_GITHUB_STATE"] = str(state_path)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        cargo = bin_dir / "cargo"
        cargo.write_text("#!/bin/sh\n"
                         "if [ \"${3:-}\" = bench_hidden ] && [ -n \"${READY_FIFO:-}\" ]; then\n"
                         "  printf 'ready\\n' > \"$READY_FIFO\"\n"
                         "  exec /bin/cat > /dev/null\n"
                         "fi\n"
                         "printf 'test result: ok. 4 passed; 0 failed; 0 ignored\\n'\n")
        cargo.chmod(0o755)
        self.env["PATH"] = f"{bin_dir}{os.pathsep}{self.env['PATH']}"

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, env=self.env, check=True,
                              capture_output=True, text=True).stdout.strip()

    def test_success_leaves_agent_checkout_unchanged(self):
        result = subprocess.run(["python3", str(CHECKER)], cwd=self.repo, env=self.env,
                                capture_output=True, text=True, timeout=30, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_symlink_target_fails_and_is_not_written(self):
        outside = self.root / "outside"
        outside.write_text("sentinel\n")
        target = self.repo / "src/pitchfork_toml.rs"
        target.unlink()
        target.symlink_to(outside)
        self.git("add", "src/pitchfork_toml.rs")
        self.git("commit", "-qm", "symlink target")
        result = subprocess.run(["python3", str(CHECKER)], cwd=self.repo, env=self.env,
                                capture_output=True, text=True, timeout=30, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("hidden-test target is missing or a symlink", result.stdout + result.stderr)
        self.assertEqual(outside.read_text(), "sentinel\n")
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_deleted_target_fails(self):
        self.git("rm", "src/pitchfork_toml.rs")
        self.git("commit", "-qm", "delete target")
        result = subprocess.run(["python3", str(CHECKER)], cwd=self.repo, env=self.env,
                                capture_output=True, text=True, timeout=30, check=False)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("hidden-test target is missing", result.stdout)

    def test_timeout_with_a_failure_is_failed_otherwise_unverified(self):
        wrapper = """import runpy, subprocess, sys
original = subprocess.run
def run(args, **kwargs):
    if args[0] == "cargo":
        raise subprocess.TimeoutExpired(args, 1500, output=sys.argv[2].encode())
    return original(args, **kwargs)
subprocess.run = run
runpy.run_path(sys.argv[1], run_name="__main__")
"""
        for output, code in (("test regression ... FAILED\n", 1), ("running tests\n", 3)):
            with self.subTest(output=output):
                result = subprocess.run(["python3", "-c", wrapper, str(CHECKER), output], cwd=self.repo,
                                        env=self.env, capture_output=True, text=True, timeout=30, check=False)
                self.assertEqual(result.returncode, code, result.stdout + result.stderr)

    def test_killed_checker_does_not_change_agent_checkout(self):
        fifo = self.root / "ready"
        os.mkfifo(fifo)
        read_fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
        self.addCleanup(os.close, read_fd)
        process = subprocess.Popen(["python3", str(CHECKER)], cwd=self.repo,
                                   env={**self.env, "READY_FIFO": str(fifo)},
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        try:
            ready, _, _ = select.select([read_fd], [], [], 30)
            self.assertTrue(ready, "hidden check did not start")
            self.assertEqual(os.read(read_fd, 64), b"ready\n")
        finally:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=10)
        self.assertEqual(self.git("status", "--porcelain"), "")


if __name__ == "__main__":
    unittest.main()
