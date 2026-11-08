import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


TASKS = Path(__file__).resolve().parents[1] / "tasks"


class CheckHelpersTest(unittest.TestCase):
    def test_successful_unittest_with_zero_tests_is_not_a_pass(self):
        spec = importlib.util.spec_from_file_location("bench_check_helpers", TASKS / "_check.py")
        helper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(helper)
        empty = subprocess.CompletedProcess([], 0, stdout="", stderr="Ran 0 tests in 0.000s\n\nOK\n")
        with mock.patch.object(helper.subprocess, "run", return_value=empty):
            passed, output = helper.tests_pass("discover")
        self.assertFalse(passed)
        self.assertIn("Ran 0 tests", output)

    def test_unittest_discovery_requires_at_least_one_test(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            tests = work / "tests"
            tests.mkdir()
            code = "import json; from _check import tests_pass; print(json.dumps(tests_pass('discover', '-s', 'tests')))"

            def run():
                result = subprocess.run(
                    [sys.executable, "-c", code], cwd=work,
                    env={**os.environ, "PYTHONPATH": str(TASKS)},
                    capture_output=True, text=True, check=True,
                )
                return json.loads(result.stdout)

            passed, output = run()
            self.assertFalse(passed)
            self.assertIn("Ran 0 tests", output)
            (tests / "test_one.py").write_text(
                "import unittest\nclass OneTest(unittest.TestCase):\n"
                "    def test_one(self):\n        self.assertTrue(True)\n"
            )
            passed, output = run()
            self.assertTrue(passed, output)
            self.assertIn("Ran 1 test", output)

    def test_subprocess_failures_retain_the_first_and_last_diagnostics(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            (work / "test_long.py").write_text(
                "import unittest\n"
                "class LongFailure(unittest.TestCase):\n"
                "    def test_failure(self):\n"
                "        self.fail('first-diagnostic' + 'x' * 4000 + 'last-diagnostic')\n"
            )
            code = (
                "import json; from _check import tests_pass, run_python; "
                "a = tests_pass('test_long'); "
                "b = run_python(\"import sys; print('first-diagnostic' + 'x' * 4000 + "
                "'last-diagnostic'); sys.exit(1)\"); "
                "print(json.dumps([a, b]))"
            )
            result = subprocess.run(
                [sys.executable, "-c", code], cwd=work,
                env={**os.environ, "PYTHONPATH": str(TASKS)},
                capture_output=True, text=True, check=True,
            )
            for passed, output in json.loads(result.stdout):
                self.assertFalse(passed)
                self.assertIn("first-diagnostic", output)
                self.assertIn("last-diagnostic", output)

    def test_dirty_keeps_the_first_character_of_unstaged_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid",
                   "PYTHONPATH": str(TASKS)}

            def git(*args):
                subprocess.run(["git", *args], cwd=work, env=env, check=True, capture_output=True)

            git("init", "-q")
            (work / "README.md").write_text("before\n")
            (work / "second.py").write_text("before\n")
            git("add", "README.md", "second.py")
            git("commit", "-qm", "base")
            (work / "README.md").write_text("after\n")
            (work / "second.py").write_text("after\n")
            git("add", "second.py")
            result = subprocess.run(
                [sys.executable, "-c", "import json; from _check import dirty; print(json.dumps(dirty()))"],
                cwd=work, env=env, check=True, capture_output=True, text=True,
            )
            self.assertEqual(json.loads(result.stdout), ["README.md", "second.py"])


if __name__ == "__main__":
    unittest.main()
