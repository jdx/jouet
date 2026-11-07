import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CHECKER = Path(__file__).resolve().parents[1] / "tasks/click-short-help/check.py"


class ClickCheckerTest(unittest.TestCase):
    def test_timeout_keeps_captured_output_and_removes_checkout(self):
        spec = importlib.util.spec_from_file_location("click_checker", CHECKER)
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
        ok = subprocess.CompletedProcess([], 0, stdout="", stderr="")
        timeout = subprocess.TimeoutExpired("pytest", 120, output=b"stdout\xff", stderr=b"stderr\n")
        with mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[ok, timeout, ok]) as process:
            checker.check_committed_tests([])
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("timed out", checker.UNVERIFIED[0])
            self.assertIn("stdout\ufffdstderr", checker.UNVERIFIED[0])
            self.assertEqual(process.call_count, 3)

    def test_ruff_failures_are_compared_and_unavailable_checks_are_unverified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            source = work / "source with spaces.py"
            source.write_text("value = 1\n")
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            git("add", source.name)
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            source.write_text("value = 2\n")
            git("add", source.name)
            git("commit", "-qm", "fix")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            (bin_dir / "git").symlink_to(shutil.which("git"))
            python = bin_dir / "python3"
            python.write_text(
                '#!/bin/sh\n'
                'if [ "$1" != -m ]; then exit "${FAKE_BEHAVIOR_EXIT:-0}"; fi\n'
                'for arg in "$@"; do case "$arg" in --junitxml=*) report=${arg#--junitxml=};; esac; done\n'
                'case "${FAKE_PYTEST:-pass}" in\n'
                '  fail) printf "<testsuite><testcase><failure/></testcase></testsuite>" > "$report"; exit 1;;\n'
                '  skip) printf "<testsuite><testcase><skipped/></testcase></testsuite>" > "$report"; exit 0;;\n'
                '  missing) exit 0;;\n'
                '  helper) [ -f tests/helper.py ] || exit 2;;\n'
                'esac\n'
                'printf "<testsuite><testcase/></testsuite>" > "$report"\n'
            )
            python.chmod(0o755)
            ruff = bin_dir / "ruff"
            ruff.write_text(
                '#!/bin/sh\n'
                '[ "$1" = check ] && exit 0\n'
                'case "$*" in *tests/test_*) exit 0;; esac\n'
                'case "$*" in *--stdin-filename*) exit "${FAKE_BASE_EXIT:-0}";; esac\n'
                'exit "${FAKE_FORMAT_EXIT:-0}"\n'
            )
            ruff.chmod(0o755)
            env.update({"PATH": str(bin_dir), "BENCH_HEAD_SHA": base})

            def check(**extra):
                return subprocess.run([sys.executable, "-I", str(CHECKER)], cwd=work,
                                      env={**env, **extra}, capture_output=True, text=True, check=False)

            result = check()
            self.assertEqual(result.returncode, 1)
            self.assertIn("no committed Python test file", result.stdout)
            (work / "tests").mkdir()
            (work / "tests/test_change.py").write_text("def test_change(): pass\n")
            git("add", "tests/test_change.py")
            git("commit", "-qm", "test")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = check(FAKE_PYTEST="fail")
            self.assertEqual(result.returncode, 1)
            self.assertIn("committed tests failed", result.stdout)
            for mode in ("skip", "missing"):
                result = check(FAKE_PYTEST=mode)
                self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            (work / "tests/helper.py").write_text("untracked\n")
            self.assertEqual(check(FAKE_PYTEST="helper").returncode, 3)
            test_commit = git("rev-parse", "HEAD")
            git("mv", "tests/test_change.py", "tests/test_renamed.py")
            git("commit", "-qm", "rename test")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            git("rm", "tests/test_renamed.py")
            git("commit", "-qm", "delete test")
            result = check()
            self.assertEqual(result.returncode, 1)
            self.assertIn("no committed Python test file", result.stdout)
            git("checkout", test_commit, "--", "tests/test_change.py")
            git("commit", "-qm", "restore test")
            result = check(FAKE_FORMAT_EXIT="1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("Ruff format fails on source with spaces.py", result.stdout)
            result = check(FAKE_FORMAT_EXIT="1", FAKE_BASE_EXIT="1")
            self.assertEqual(result.returncode, 3)
            self.assertTrue(result.stdout.rstrip().endswith("JOUET_BENCH_UNVERIFIED_V1"))
            result = check(FAKE_FORMAT_EXIT="2")
            self.assertEqual(result.returncode, 3)
            self.assertTrue(result.stdout.rstrip().endswith("JOUET_BENCH_UNVERIFIED_V1"))
            added = work / "new.py"
            added.write_text("value = 3\n")
            git("add", added.name)
            git("commit", "-qm", "add file")
            result = check(FAKE_FORMAT_EXIT="1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("Ruff format fails on new file new.py", result.stdout)
            ruff.unlink()
            result = check()
            self.assertEqual(result.returncode, 3)
            self.assertTrue(result.stdout.rstrip().endswith("JOUET_BENCH_UNVERIFIED_V1"))
            result = check(FAKE_BEHAVIOR_EXIT="1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("short-help cases failed", result.stdout)


if __name__ == "__main__":
    unittest.main()
