import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

TASKS = Path(__file__).resolve().parents[1] / "tasks"
sys.path.insert(0, str(TASKS))
spec = importlib.util.spec_from_file_location(
    "fnox_bats_check", TASKS / "fnox-editor-args" / "bats_check.py"
)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class FnoxBatsTest(unittest.TestCase):
    def test_realistic_tap_requires_complete_success(self):
        self.assertIsNone(checker.inspect_tap("1..2\nok 1 first\nok 2 second\n", 0))
        cases = [
            ("", 0),
            ("1..0\n", 0),
            ("1..2\nok 1 first\n", 0),
            ("1..2\nok 1 first\nok 1 duplicate\n", 0),
            ("1..1\nok 1 first # skip unavailable\n", 0),
            ("1..1\nok 1 first # TODO later\n", 0),
            ("1..1\nnot ok 1 first\n", 1),
            ("1..1\nok 1 first\n", 1),
            ("1..1\n1..1\nok 1 first\n", 0),
        ]
        for output, status in cases:
            with self.subTest(output=output, status=status):
                self.assertIsNotNone(checker.inspect_tap(output, status))
        self.assertIsNone(checker.inspect_tap("1..2\nok 1 first\nok 2 second\n", 0, ["first", "second"]))
        self.assertIsNotNone(checker.inspect_tap("1..2\nok 1 first\nok 2 other\n", 0, ["first", "second"]))
        self.assertEqual(
            checker.inspect_tap("1..2\nnot ok 1 first\nok 2 second # skip unavailable\n", 1, ["first", "second"]),
            "a committed regression test failed",
        )

    def test_complete_run_executes_both_committed_tests(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        diff = subprocess.CompletedProcess([], 0, '+@test "first" {\n+@test "second" {\n', "")
        clean = subprocess.CompletedProcess([], 0, "", "")
        report = subprocess.CompletedProcess([], 0, "1..2\nok 1 first\nok 2 second\n", "")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, diff, clean, report]), \
                contextlib.redirect_stdout(io.StringIO()):
            checker.check_bats()
            self.assertEqual(checker.UNVERIFIED, [])

    def test_bats_failure_and_skip_are_unverified(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        diff = subprocess.CompletedProcess([], 0, '+@test "first" {\n+@test "second" {\n', "")
        clean = subprocess.CompletedProcess([], 0, "", "")
        report = subprocess.CompletedProcess(
            [], 1, "1..2\nok 1 editor # skip unavailable\nnot ok 2 keychain\n", ""
        )
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, diff, clean, report]), \
                contextlib.redirect_stdout(io.StringIO()):
            checker.check_bats()
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("skipped", checker.UNVERIFIED[0])

    def test_untracked_files_cannot_make_tests_pass(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        diff = subprocess.CompletedProcess([], 0, '+@test "first" {\n+@test "second" {\n', "")
        untracked = subprocess.CompletedProcess([], 0, "test/helper.bash\n", "")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, diff, untracked]) as run:
            checker.check_bats()
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("untracked", checker.UNVERIFIED[0])
            self.assertEqual(run.call_count, 3)


if __name__ == "__main__":
    unittest.main()
