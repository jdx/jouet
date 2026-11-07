import importlib.util
import json
import subprocess
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


if __name__ == "__main__":
    unittest.main()
