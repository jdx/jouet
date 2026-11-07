import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

CHECKER = Path(__file__).resolve().parents[1] / "tasks/usage-heading-order/check.py"


class UsageEvidenceTest(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("usage_checker", CHECKER)
        self.checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.checker)
        self.checker.UNVERIFIED.clear()
        self.problems = self.checker.expect.__globals__["PROBLEMS"]
        self.problems.clear()

    def test_only_complete_nonempty_targets_are_accepted(self):
        samples = [
            ("running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", 0, False),
            ("running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", 0, True),
            ("running 2 tests\ntest result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;", 0, True),
            ("running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;", 0, True),
            ("running 2 tests\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", 0, True),
            ("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", 0, True),
            ("running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", 1, True),
        ]
        for output, code, unverified in samples:
            with self.subTest(output=output, code=code):
                self.checker.UNVERIFIED.clear()
                self.problems.clear()
                proc = subprocess.CompletedProcess(["cargo"], code, stdout=output)
                with mock.patch.object(self.checker.subprocess, "run", return_value=proc), \
                        mock.patch.object(self.checker.time, "monotonic", return_value=0):
                    self.checker.run("heading_help", "committed test", 100, Path("checkout"))
                self.assertEqual(bool(self.checker.UNVERIFIED), unverified)
                self.assertFalse(self.problems)

    def test_a_reported_failure_fails_the_check(self):
        output = "running 1 test\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;"
        proc = subprocess.CompletedProcess(["cargo"], 101, stdout=output)
        with mock.patch.object(self.checker.subprocess, "run", return_value=proc), \
                mock.patch.object(self.checker.time, "monotonic", return_value=0):
            self.checker.run("heading_help", "committed test", 100, Path("checkout"))
        self.assertTrue(self.problems)
        self.assertFalse(self.checker.UNVERIFIED)

    def test_a_failure_with_an_ignored_test_is_still_a_failure(self):
        output = "running 2 tests\ntest result: FAILED. 0 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out;"
        proc = subprocess.CompletedProcess(["cargo"], 101, stdout=output)
        with mock.patch.object(self.checker.subprocess, "run", return_value=proc), \
                mock.patch.object(self.checker.time, "monotonic", return_value=0):
            self.checker.run("heading_help", "committed test", 100, Path("checkout"))
        self.assertTrue(self.problems)
        self.assertTrue(self.checker.UNVERIFIED)

    def test_expired_budget_does_not_start_a_test(self):
        with mock.patch.object(self.checker.subprocess, "run") as run, \
                mock.patch.object(self.checker.time, "monotonic", return_value=101):
            self.checker.run("heading_help", "committed test", 100, Path("checkout"))
        run.assert_not_called()
        self.assertTrue(self.checker.UNVERIFIED)

    def test_timeout_preserves_captured_output_and_is_unverified(self):
        output = io.StringIO()
        error = subprocess.TimeoutExpired("cargo", 1, output=b"before timeout\n")
        with redirect_stdout(output), mock.patch.object(self.checker.subprocess, "run", side_effect=error), \
                mock.patch.object(self.checker.time, "monotonic", return_value=0):
            self.checker.run("heading_help", "committed test", 100, Path("checkout"))
        self.assertIn("before timeout", output.getvalue())
        self.assertTrue(self.checker.UNVERIFIED)
        self.assertFalse(self.problems)


class UsageCheckerTest(unittest.TestCase):
    def test_requires_a_commit_and_no_tracked_changes_before_hidden_test(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            tests = work / "conformance/tests"
            tests.mkdir(parents=True)
            (tests / "heading_help.rs").write_text("original test\n")
            source = work / "source.rs"
            source.write_text("before\n")
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            git("add", "source.rs", "conformance/tests/heading_help.rs")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            cargo = bin_dir / "cargo"
            cargo.write_text(
                '#!/bin/sh\n'
                'printf "%s %s %s\\n" "$5" "$(git hash-object "conformance/tests/$5.rs")" '
                '"$(git hash-object source.rs)" >> "$TRACE"\n'
                'if [ "$5" = heading_help ]; then printf "changed by test\\n" > source.rs; fi\n'
                'printf "running 1 test\\ntest example ... ok\\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\\n"\n'
            )
            cargo.chmod(0o755)
            trace = root / "trace"
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_HEAD_SHA": base,
                        "TRACE": str(trace)})

            def check():
                return subprocess.run([sys.executable, "-I", str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True, check=False)

            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("nothing was committed", result.stdout)
            git("restore", "conformance/tests/heading_help.rs")
            source.write_text("fixed\n")
            git("add", "source.rs")
            git("commit", "-qm", "fix")
            source.write_text("uncommitted changes\n")
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("changes remain uncommitted", result.stdout)
            git("restore", "source.rs", "conformance/tests/heading_help.rs")
            (work / "uncommitted.rs").write_text("untracked\n")
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("changes remain uncommitted", result.stdout)
            (work / "uncommitted.rs").unlink()
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("no changed conformance test target to verify", result.stdout)
            (tests / "heading_help.rs").write_text("agent test\n")
            git("add", "conformance/tests/heading_help.rs")
            git("commit", "-qm", "agent test")
            agent_hash = git("hash-object", "conformance/tests/heading_help.rs")
            source_hash = git("hash-object", "source.rs")
            trace.write_text("")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            runs = [line.split() for line in trace.read_text().splitlines()]
            self.assertEqual(len(runs), 2)
            self.assertEqual(runs[0], ["heading_help", agent_hash, source_hash])
            self.assertTrue(runs[1][0].startswith("jouet_hidden_"))
            self.assertNotEqual(runs[1][1], agent_hash)
            self.assertEqual(runs[1][2], source_hash)
            self.assertEqual((tests / "heading_help.rs").read_text(), "agent test\n")
            self.assertEqual(source.read_text(), "fixed\n")
            self.assertFalse(list(tests.glob("jouet_hidden_*.rs")))
            self.assertEqual(sum(line.startswith("worktree ") for line in git("worktree", "list", "--porcelain").splitlines()), 1)


if __name__ == "__main__":
    unittest.main()
