import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CHECKER = Path(__file__).resolve().parents[1] / "tasks/requests-proxied-files/check.py"
spec = importlib.util.spec_from_file_location("requests_checker", CHECKER)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class RequestsCheckerTest(unittest.TestCase):
    def test_report_totals_must_match_executed_cases(self):
        for report in (
            '<testsuite tests="2" failures="0" errors="0" skipped="0"><testcase/></testsuite>',
            '<testsuite tests="1" failures="0" errors="0" skipped="0"><testcase/><testcase/></testsuite>',
            '<testsuite tests="1" failures="0" errors="0" skipped="0"><testcase><failure/></testcase></testsuite>',
            ('<testsuite tests="1" failures="1" errors="0" skipped="1">'
             '<testcase><failure/><skipped/></testcase></testsuite>'),
            '<testsuite><testcase/></testsuite>',
        ):
            with self.subTest(report=report):
                unverified, failures = self.check_report(report)
                self.assertTrue(unverified)
                self.assertFalse(failures)

    def test_skipped_case_details_are_reported(self):
        report = ('<testsuite tests="1" failures="0" errors="0" skipped="1">'
                  '<testcase classname="tests.test_requests.TestRequests" name="test_https_warnings">'
                  '<skipped type="pytest.skip" message="urllib3 warning unavailable"/>'
                  '</testcase></testsuite>')
        unverified, failures = self.check_report(report)
        self.assertFalse(failures)
        self.assertIn("test_https_warnings", unverified[0])
        self.assertIn("urllib3 warning unavailable", unverified[0])

    def test_long_skip_list_is_explicitly_truncated(self):
        report = ('<testsuite tests="11" failures="0" errors="0" skipped="11">'
                  + "".join(f'<testcase name="test_{n}"><skipped message="reason"/></testcase>' for n in range(11))
                  + '</testsuite>')
        unverified, _ = self.check_report(report)
        self.assertIn("test_0", unverified[0])
        self.assertIn("1 more cases omitted", unverified[0])

    def check_report(self, xml, exit_code=0):
        def run(command, **kwargs):
            report = next(arg.split("=", 1)[1] for arg in command if arg.startswith("--junitxml="))
            if xml is not None:
                Path(report).write_text(xml)
            self.assertEqual(command[-2:], ["--", "tests/test_added.py"])
            return subprocess.CompletedProcess(command, exit_code, stdout="output", stderr="")

        with mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker, "expect") as expect, \
                mock.patch.object(checker.subprocess, "run", side_effect=run):
            checker.check_tests(["tests/test_added.py"])
            return list(checker.UNVERIFIED), expect.call_args_list

    def test_pass_requires_a_complete_report(self):
        self.assertEqual(self.check_report(
            '<testsuites><testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase/></testsuite></testsuites>'
        ), ([], []))
        for report in (None, "<testsuites>", "<testsuites/>",
                       ('<testsuites><testsuite tests="1" failures="0" errors="0" skipped="1">'
                        '<testcase><skipped/></testcase></testsuite></testsuites>')):
            with self.subTest(report=report):
                unverified, failures = self.check_report(report)
                self.assertTrue(unverified)
                self.assertFalse(failures)

    def test_failure_and_runner_error_are_distinct(self):
        report = ('<testsuites><testsuite tests="1" failures="1" errors="0" skipped="0">'
                  '<testcase><failure/></testcase></testsuite></testsuites>')
        for code in (0, 1):
            with self.subTest(code=code):
                unverified, failures = self.check_report(report, code)
                self.assertFalse(unverified)
                self.assertEqual(failures[0].args[0], False)
        for report, code in ((None, 1), (None, 2), ("<broken", 1),
                             (('<testsuite tests="1" failures="0" errors="1" skipped="0">'
                               '<testcase><error/></testcase></testsuite>'), 1),
                             (('<testsuite tests="1" failures="0" errors="0" skipped="0">'
                               '<testcase/></testsuite>'), 1)):
            with self.subTest(report=report, code=code):
                unverified, failures = self.check_report(report, code)
                self.assertTrue(unverified)
                self.assertFalse(failures)

    def test_each_file_must_collect_tests(self):
        def run(command, **kwargs):
            report = next(arg.split("=", 1)[1] for arg in command if arg.startswith("--junitxml="))
            case = "<testcase/>" if command[-1] == "tests/test_one.py" else ""
            Path(report).write_text(
                f'<testsuite tests="{int(bool(case))}" failures="0" errors="0" skipped="0">{case}</testsuite>'
            )
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

        with mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=run):
            checker.check_tests(["tests/test_one.py", "tests/test_empty.py"])
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("tests/test_empty.py", checker.UNVERIFIED[0])

    def test_timeout_preserves_partial_output(self):
        error = subprocess.TimeoutExpired("pytest", 1, output=b"some output\xff", stderr=b"some error")
        with mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=error):
            checker.check_tests(["tests/test_slow.py"])
            self.assertIn("some output\ufffd", checker.UNVERIFIED[0])
            self.assertIn("some error", checker.UNVERIFIED[0])

    def test_requires_a_committed_test_file_that_still_exists(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            (work / "source.py").write_text("before\n")
            (work / "tests").mkdir()
            (work / "tests/test_existing.py").write_text("def test_existing(): pass\n")
            git("add", "source.py", "tests/test_existing.py")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (work / "source.py").write_text("after\n")
            git("add", "source.py")
            git("commit", "-qm", "fix")

            # The independent multipart cases have their own validation. Here
            # they succeed so this test isolates the committed-test requirement.
            bin_dir = root / "bin"
            bin_dir.mkdir()
            fake_python = bin_dir / "python3"
            fake_python.write_text("#!/bin/sh\nfor arg do\ncase \"$arg\" in --junitxml=*) "
                                   "printf '<testsuite tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\"><testcase/></testsuite>' > \"${arg#*=}\";; esac\n"
                                   "done\nexit 0\n")
            fake_python.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_HEAD_SHA": base})

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env, check=False,
                                      capture_output=True, text=True)

            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("no committed test file was changed", result.stdout)
            existing = work / "tests/test_existing.py"
            existing.unlink()
            git("add", "tests/test_existing.py")
            git("commit", "-qm", "delete test")
            existing.write_text("def test_existing(): pass\n")
            self.assertNotEqual(check().returncode, 0, "an untracked replacement is not committed")
            existing.unlink()
            git("checkout", base, "--", "tests/test_existing.py")
            git("commit", "-qm", "restore test")
            added = work / "tests/test_upload.py"
            added.write_text("def test_upload(): pass\n")
            self.assertNotEqual(check().returncode, 0, "an untracked test is not committed")
            git("add", "tests/test_upload.py")
            git("commit", "-qm", "add test")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            added.unlink()
            git("add", "tests/test_upload.py")
            git("commit", "-qm", "remove test")
            self.assertNotEqual(check().returncode, 0, "a deleted test is not evidence")
            (work / "tests/test_existing.py").write_text("def test_existing(): assert True\n")
            git("add", "tests/test_existing.py")
            git("commit", "-qm", "extend test")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
