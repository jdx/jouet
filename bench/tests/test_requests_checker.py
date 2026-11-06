import os
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


CHECKER = Path(__file__).resolve().parents[1] / "tasks/requests-proxied-files/check.py"
spec = importlib.util.spec_from_file_location("requests_checker", CHECKER)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class RequestsCheckerTest(unittest.TestCase):
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
        self.assertEqual(self.check_report("<testsuites><testsuite><testcase/></testsuite></testsuites>"), ([], []))
        for report in (None, "<testsuites>", "<testsuites/>",
                       "<testsuites><testsuite><testcase><skipped/></testcase></testsuite></testsuites>"):
            with self.subTest(report=report):
                unverified, failures = self.check_report(report)
                self.assertTrue(unverified)
                self.assertFalse(failures)

    def test_failure_and_runner_error_are_distinct(self):
        for report, code in ((None, 1),
                             ("<testsuites><testsuite><testcase><failure/></testcase></testsuite></testsuites>", 0)):
            with self.subTest(code=code):
                unverified, failures = self.check_report(report, code)
                self.assertFalse(unverified)
                self.assertEqual(failures[0].args[0], False)
        unverified, failures = self.check_report(None, 2)
        self.assertTrue(unverified)
        self.assertFalse(failures)

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
                                   "printf '<testsuite><testcase/></testsuite>' > \"${arg#*=}\";; esac\n"
                                   "done\nexit 0\n")
            fake_python.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_HEAD_SHA": base})

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env,
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
