import importlib.util
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


CHECKER = Path(__file__).resolve().parents[1] / "tasks/commander-output-config/check.py"


class CommanderCheckerTest(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("commander_checker", CHECKER)
        self.checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.checker)
        self.checker.UNVERIFIED.clear()
        self.checker.expect.__globals__["PROBLEMS"].clear()

    def test_jest_report_must_show_complete_passing_tests(self):
        reports = [
            ({"numTotalTests": 1, "numPassedTests": 1, "numFailedTests": 0, "numPendingTests": 0}, False),
            ({"numTotalTests": 2, "numPassedTests": 1, "numFailedTests": 0, "numPendingTests": 1}, True),
            ({"numTotalTests": 2, "numPassedTests": 1, "numFailedTests": 0, "numPendingTests": 0}, True),
            ({"numTotalTests": 1, "numPassedTests": 1, "numFailedTests": 0}, True),
        ]
        for report, unverified in reports:
            with self.subTest(report=report), tempfile.TemporaryDirectory() as directory:
                self.checker.UNVERIFIED.clear()
                root = Path(directory)
                statuses = (["passed"] * report["numPassedTests"] + ["failed"] * report["numFailedTests"]
                            + ["pending"] * report.get("numPendingTests", 0))
                report["testResults"] = [{"name": str(root / "tests/example.test.js"),
                                          "assertionResults": [{"status": status} for status in statuses]}]
                (root / "jouet-bench-jest-report.json").write_text(json.dumps(report))
                result = subprocess.CompletedProcess(["jest"], 0, stdout="", stderr="")
                with mock.patch.object(self.checker.subprocess, "run", return_value=result):
                    self.checker.run_tests(root, ["tests/example.test.js"], "committed tests")
                self.assertEqual(bool(self.checker.UNVERIFIED), unverified)

    def test_missing_or_failed_jest_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(self.checker.subprocess, "run", side_effect=OSError("missing")):
                self.checker.run_tests(Path(directory), ["tests/example.test.js"], "committed tests")
            self.assertIn("could not run", self.checker.UNVERIFIED[0])

    def test_timeout_keeps_jest_output(self):
        output = io.StringIO()
        error = subprocess.TimeoutExpired("jest", 240, output=b"stdout before hang\n", stderr=b"stderr before hang\n")
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(output), \
                mock.patch.object(self.checker.subprocess, "run", side_effect=error):
            self.checker.run_tests(Path(directory), ["tests/example.test.js"], "committed tests")
        self.assertIn("stdout before hang", output.getvalue())
        self.assertIn("stderr before hang", output.getvalue())
        self.assertTrue(self.checker.UNVERIFIED)

    def test_absent_suite_cannot_pass_on_other_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = {"numTotalTests": 1, "numPassedTests": 1, "numFailedTests": 0,
                      "numPendingTests": 0, "testResults": [{"name": str(root / "tests/other.test.js"),
                      "assertionResults": [{"status": "passed"}]}]}
            (root / "jouet-bench-jest-report.json").write_text(json.dumps(report))
            result = subprocess.CompletedProcess(["jest"], 0, stdout="", stderr="")
            with mock.patch.object(self.checker.subprocess, "run", return_value=result):
                self.checker.run_tests(root, ["tests/example.test.js"], "committed tests")
            self.assertIn("missing test suites", self.checker.UNVERIFIED[0])

    def test_original_tests_include_the_old_path_of_a_rename(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, capture_output=True,
                                      text=True, check=True).stdout.strip()

            git("init", "-q")
            (work / "tests").mkdir()
            (work / "tests/old.test.js").write_text("original test\n")
            git("add", "tests")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            git("mv", "tests/old.test.js", "tests/new.test.js")
            git("commit", "-qm", "rename")
            previous = Path.cwd()
            try:
                os.chdir(work)
                with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": base}):
                    self.assertEqual(self.checker.original_tests(), {"tests/old.test.js": b"original test\n"})
            finally:
                os.chdir(previous)

    def test_committed_and_hidden_tests_use_isolated_checkouts(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory) / "work"
            work.mkdir()
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, capture_output=True,
                                      text=True, check=True).stdout.strip()

            git("init", "-q")
            (work / "tests").mkdir()
            test = work / "tests/command.configureOutput.test.js"
            test.write_text("base test\n")
            source = work / "source.js"
            source.write_text("base source\n")
            git("add", "tests", "source.js")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            binary = work / "node_modules/.bin"
            binary.mkdir(parents=True)
            jest = binary / "jest"
            jest.write_text(
                '#!/bin/sh\n'
                'for arg in "$@"; do case "$arg" in --outputFile=*) report=${arg#--outputFile=};; esac; done\n'
                'hash=$(git hash-object tests/command.configureOutput.test.js)\n'
                'printf "%s|%s\\n" "$PWD" "$hash" >> "$TRACE"\n'
                'if [ "$(cat source.js)" = "regressed source" ] && [ "$(cat tests/command.configureOutput.test.js)" = "base test" ]; then passed=0; failed=1; status=failed; else passed=1; failed=0; status=passed; fi\n'
                'printf "{\\"numTotalTests\\":1,\\"numPassedTests\\":%s,\\"numFailedTests\\":%s,\\"numPendingTests\\":0,\\"testResults\\":[{\\"name\\":\\"%s/tests/command.configureOutput.test.js\\",\\"assertionResults\\":[{\\"status\\":\\"%s\\"}]}]}\\n" "$passed" "$failed" "$PWD" "$status" > "$report"\n'
                'exit "$failed"\n'
            )
            jest.chmod(0o755)
            trace = Path(directory) / "trace"
            env["TRACE"] = str(trace)

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True, check=False)

            self.assertIn("nothing was committed", check().stdout)
            source.write_text("fixed source\n")
            git("add", "source.js")
            git("commit", "-qm", "fix")
            self.assertIn("no committed Commander test file was changed", check().stdout)
            test.write_text("agent test\n")
            git("add", "tests")
            git("commit", "-qm", "test")
            original_hash = git("hash-object", "tests/command.configureOutput.test.js")
            trace.write_text("")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            entries = [line.split("|", 1) for line in trace.read_text().splitlines()]
            self.assertEqual(len(entries), 3)
            self.assertEqual(entries[0][1], original_hash)
            self.assertNotEqual(entries[1][1], original_hash)
            self.assertNotEqual(entries[2][1], original_hash)
            self.assertNotEqual(entries[0][0], entries[1][0])
            self.assertEqual(test.read_text(), "agent test\n")
            source.write_text("regressed source\n")
            git("add", "source.js")
            git("commit", "-qm", "regress original behavior")
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("original tests on the fix reported 1 failing tests", result.stdout)
            self.assertEqual(test.read_text(), "agent test\n")
            source.write_text("fixed source\n")
            git("add", "source.js")
            git("commit", "-qm", "restore original behavior")
            git("rm", "-q", "tests/command.configureOutput.test.js")
            (work / "tests").mkdir(exist_ok=True)
            (work / "tests/other.test.js").write_text("other test\n")
            git("add", "tests")
            git("commit", "-qm", "delete hidden target")
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("hidden-test target was removed", result.stdout)
            git("rm", "-qr", "tests")
            outside = Path(directory) / "outside"
            outside.mkdir()
            (outside / test.name).write_text("outside file\n")
            (work / "tests").symlink_to(outside, target_is_directory=True)
            git("add", "tests")
            git("commit", "-qm", "replace test directory with symlink")
            result = check()
            self.assertIn("hidden-test target is not a regular file", result.stdout)
            self.assertEqual((outside / test.name).read_text(), "outside file\n")


if __name__ == "__main__":
    unittest.main()
