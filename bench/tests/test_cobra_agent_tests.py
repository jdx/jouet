import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

TASKS = Path(__file__).resolve().parents[1] / "tasks"
sys.path.insert(0, str(TASKS))
spec = importlib.util.spec_from_file_location("cobra_agent_tests", TASKS / "cobra-completion-args/agent_tests.py")
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)
from _check import PROBLEMS, UNVERIFIED


@unittest.skipUnless(shutil.which("go"), "Go is not installed")
class CobraAgentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.old_cwd = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self.old_cwd)
        self.env = mock.patch.dict(os.environ, {
            "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
            "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid",
            "GOPROXY": "off", "GOTOOLCHAIN": "local",
        })
        self.env.start()
        self.addCleanup(self.env.stop)
        self.git("init", "-q")
        (self.root / "go.mod").write_text("module example.invalid/cobra-bench\n\ngo 1.18\n")
        (self.root / "base_test.go").write_text('package bench\nimport "testing"\nfunc TestBase(t *testing.T) {}\n')
        self.git("add", "go.mod", "base_test.go")
        self.git("commit", "-qm", "base")
        os.environ["BENCH_HEAD_SHA"] = self.git("rev-parse", "HEAD")
        PROBLEMS.clear()
        UNVERIFIED.clear()

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, capture_output=True, text=True, check=True).stdout.strip()

    def add_test(self, body, prefix=""):
        (self.root / "agent_test.go").write_text(
            prefix + 'package bench\nimport "testing"\nfunc TestAgent(t *testing.T) { ' + body + ' }\n'
        )
        self.git("add", "agent_test.go")
        self.git("commit", "-qm", "agent test")

    def test_real_committed_test_passes(self):
        self.add_test("")
        checker.check_committed_tests()
        self.assertEqual(PROBLEMS, [])
        self.assertEqual(UNVERIFIED, [])

    def test_real_committed_failure_fails(self):
        self.add_test('t.Fatal("regression")')
        checker.check_committed_tests()
        self.assertTrue(PROBLEMS)

    def test_real_skipped_test_is_unverified(self):
        self.add_test('t.Skip("not available")')
        checker.check_committed_tests()
        self.assertFalse(PROBLEMS)
        self.assertTrue(UNVERIFIED)

    def test_skipped_subtest_is_unverified(self):
        self.add_test('t.Run("child", func(t *testing.T) { t.Skip("not available") })')
        checker.check_committed_tests()
        self.assertFalse(PROBLEMS)
        self.assertTrue(UNVERIFIED)

    def test_compile_failure_is_unverified(self):
        self.add_test("missingFunction()")
        checker.check_committed_tests()
        self.assertFalse(PROBLEMS)
        self.assertTrue(UNVERIFIED)

    def test_timeout_after_reported_failure_remains_a_failure(self):
        self.add_test("")
        real_run = subprocess.run

        def run(args, **kwargs):
            if args[:2] == ["go", "test"]:
                raise subprocess.TimeoutExpired(args, 300, output=b'{"Action":"fail","Test":"TestAgent"}\n')
            return real_run(args, **kwargs)

        with mock.patch.object(checker.subprocess, "run", side_effect=run):
            checker.check_committed_tests()
        self.assertTrue(PROBLEMS)
        self.assertTrue(UNVERIFIED)

    def test_unselected_build_tag_is_unverified(self):
        self.add_test("", "//go:build jouet_unavailable_tag\n\n")
        checker.check_committed_tests()
        self.assertFalse(PROBLEMS)
        self.assertTrue(UNVERIFIED)

    def test_missing_committed_tests_are_unverified(self):
        checker.check_committed_tests()
        self.assertFalse(PROBLEMS)
        self.assertTrue(UNVERIFIED)


if __name__ == "__main__":
    unittest.main()
