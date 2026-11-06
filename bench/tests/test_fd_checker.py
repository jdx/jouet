import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

TASKS = Path(__file__).resolve().parents[1] / "tasks"


class FdCheckerTest(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "process groups are required")
    def test_timed_out_check_stops_its_child(self):
        spec = importlib.util.spec_from_file_location("fd_checker", TASKS / "fd-unrestricted-flags/check.py")
        checker = importlib.util.module_from_spec(spec)
        with mock.patch.dict(sys.modules), mock.patch.object(sys, "path", sys.path.copy()):
            sys.modules.pop("_check", None)
            spec.loader.exec_module(checker)
            with tempfile.TemporaryDirectory() as directory:
                marker = Path(directory) / "survived"
                checker.DEADLINE = time.monotonic() + 0.1
                result = checker.run(["/bin/sh", "-c", f'(sleep 1; touch "{marker}") & wait'],
                                     Path(directory), os.environ.copy())
                self.assertIsNone(result)
                subprocess.run(["sleep", "1"], check=True)
                self.assertFalse(marker.exists())

    def test_requires_committed_tests_and_runs_in_a_clean_checkout(self):
        with tempfile.TemporaryDirectory(prefix="fd-checker-test-") as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            task = root / "tasks/fd-unrestricted-flags"
            task.mkdir(parents=True)
            shutil.copyfile(TASKS / "_check.py", task.parent / "_check.py")
            shutil.copyfile(TASKS / "fd-unrestricted-flags/check.py", task / "check.py")
            redirected = {"GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
                          "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"}
            env = {key: value for key, value in os.environ.items() if key not in redirected}
            env.update({"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                        "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                        "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"})

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            (work / "src").mkdir()
            (work / "src/main.rs").write_text("before\n")
            (work / "tests").mkdir()
            (work / "tests/tests.rs").write_text("// existing tests\n")
            git("add", "src", "tests")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            binary = root / "bin"
            binary.mkdir()
            real_git = shutil.which("git", path=env["PATH"])
            git_wrapper = binary / "git"
            git_wrapper.write_text(
                '#!/bin/sh\n'
                'if [ "${FAIL_CLEANUP:-}" = 1 ] && [ "$1" = worktree ] && [ "$2" = remove ]; then\n'
                '  echo "simulated cleanup failure" >&2; exit 1\n'
                'fi\n'
                'exec "$REAL_GIT" "$@"\n'
            )
            git_wrapper.chmod(0o755)
            stub = root / "fd"
            stub.write_text(
                '#!/bin/sh\n'
                'case "$*" in\n'
                '*"-u --ignore --no-hidden"*) printf "a.txt\\n";;\n'
                '*"--ignore -u"*|*"--no-hidden -u"*|*"--ignore --no-hidden -u"*|*"-u --ignore -u"*) '
                'printf "a.txt\\n.hidden.txt\\n.ignore\\nignored.txt\\n";;\n'
                '*"-u --ignore"*) printf "a.txt\\n.hidden.txt\\n.ignore\\n";;\n'
                '*"-u --no-hidden"*) printf "a.txt\\nignored.txt\\n";;\n'
                '*) exit 1;; esac\n'
            )
            stub.chmod(0o755)
            cargo = binary / "cargo"
            cargo.write_text(
                '#!/bin/sh\n'
                'if [ -f untracked-helper ]; then exit 1; fi\n'
                'if [ "$1" = build ]; then mkdir -p "$CARGO_TARGET_DIR/debug"; '
                'cp "$FD_STUB" "$CARGO_TARGET_DIR/debug/fd"; exit 0; fi\n'
                'case "${TEST_MODE:-ok}" in\n'
                'zero) echo "test result: ok. 0 passed; 0 failed; 0 ignored";;\n'
                'ignored) echo "test result: ok. 1 passed; 0 failed; 1 ignored";;\n'
                'missing) echo "missing summary";;\n'
                'fail) echo "test result: FAILED. 0 passed; 1 failed; 0 ignored"; exit 1;;\n'
                '*) echo "test result: ok. 2 passed; 0 failed; 0 ignored";; esac\n'
            )
            cargo.chmod(0o755)
            env.update({"PATH": f"{binary}{os.pathsep}{env['PATH']}", "FD_STUB": str(stub), "REAL_GIT": real_git})

            def check(extra=None):
                return subprocess.run([sys.executable, str(task / "check.py")], cwd=work, env={**env, **(extra or {})},
                                      capture_output=True, text=True, check=False)

            self.assertIn("fix was not committed", check().stdout)
            (work / "src/main.rs").write_text("fixed\n")
            git("add", "src/main.rs")
            git("commit", "-qm", "fix")
            self.assertEqual(check().returncode, 3)
            (work / "tests/tests.rs").write_text("#[test]\nfn regression() {}\n")
            git("add", "tests/tests.rs")
            git("commit", "-qm", "regression")
            (work / "untracked-helper").write_text("must not enter the checkout\n")
            result = check({"GIT_DIR": str(root / "missing"), "GIT_WORK_TREE": str(root)})
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(len(git("worktree", "list", "--porcelain").split("worktree ")) - 1, 1)
            cleanup_result = check({"FAIL_CLEANUP": "1"})
            self.assertEqual(cleanup_result.returncode, 0, cleanup_result.stdout + cleanup_result.stderr)
            self.assertIn("could not remove committed checkout", cleanup_result.stderr)
            git("worktree", "prune")
            self.assertEqual(check({"TEST_MODE": "zero"}).returncode, 1)
            self.assertEqual(check({"TEST_MODE": "ignored"}).returncode, 3)
            self.assertEqual(check({"TEST_MODE": "missing"}).returncode, 3)
            self.assertEqual(check({"TEST_MODE": "fail"}).returncode, 1)
            (work / "src/main.rs").write_text("uncommitted\n")
            self.assertIn("tracked changes were left uncommitted", check().stdout)


if __name__ == "__main__":
    unittest.main()
