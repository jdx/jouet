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
spec = importlib.util.spec_from_file_location("hk_bats_check", TASKS / "hk-check-file-args" / "bats_check.py")
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class HkBatsTest(unittest.TestCase):
    def test_timeout_keeps_a_named_failure_but_other_timeouts_are_unverified(self):
        def result(stdout):
            return subprocess.CompletedProcess([], 0, stdout, "")

        for output, failed in [(b"1..1\nnot ok 1 regression\n", True), (b"1..1\n", False)]:
            with self.subTest(failed=failed), mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                    mock.patch.object(checker, "UNVERIFIED", []), \
                    mock.patch.object(checker, "expect") as expect, \
                    mock.patch.object(checker.subprocess, "run", side_effect=[
                        result(b"test/new.bats\0"), result(b"test/new.bats\0"), result(b""),
                        result('@test "regression" {\n'),
                        subprocess.TimeoutExpired("bats", 300, output=output),
                    ]), contextlib.redirect_stdout(io.StringIO()):
                checker.check_bats()
                if failed:
                    expect.assert_called_once_with(
                        False, "a committed Bats regression failed before the suite timed out in test/new.bats"
                    )
                    self.assertEqual(checker.UNVERIFIED, [])
                else:
                    expect.assert_not_called()
                    self.assertIn("timed out", checker.UNVERIFIED[0])


if __name__ == "__main__":
    unittest.main()
