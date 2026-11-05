import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


CHECKER = Path(__file__).resolve().parents[1] / "tasks/tornado-cancelled-handler/check.py"


class TornadoCheckerTest(unittest.TestCase):
    def test_timeout_keeps_captured_output_and_is_unverified(self):
        spec = importlib.util.spec_from_file_location("tornado_checker", CHECKER)
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
        error = subprocess.TimeoutExpired("python3", 240, output=b"stdout\xff", stderr=b"stderr\n")
        output = io.StringIO()
        with mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=error), \
                contextlib.redirect_stdout(output):
            checker.run_tests(Path("."), "tornado.test.web_test")
            self.assertEqual(output.getvalue(), "stdout\ufffd\nstderr\n")
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("timed out", checker.UNVERIFIED[0])

    def test_preserves_agent_tests_and_requires_committed_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            repo_vars = {"GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
                         "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"}
            env = {key: value for key, value in os.environ.items() if key not in repo_vars}
            env.update({"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                        "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                        "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"})

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            (work / "tornado/test").mkdir(parents=True)
            test = work / "tornado/test/web_test.py"
            test.write_text("original test\n")
            source = work / "tornado/web.py"
            source.write_text("before\n")
            git("add", "tornado")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            fake_python = bin_dir / "python3"
            fake_python.write_text(
                '#!/bin/sh\nhash=$(git hash-object tornado/test/web_test.py)\n'
                'printf "%s|%s\\n" "$PWD" "$hash" >> "$TRACE"\n'
                'if [ "${FAIL_AGENT:-}" = 1 ] && [ "$hash" = "$AGENT_HASH" ]; then exit 1; fi\n'
                'if [ "${REQUIRE_HELPER:-}" = 1 ] && [ ! -f tornado/test/helper.py ]; then exit 1; fi\n'
                'printf "Ran 1 test in 0.1s\\nOK\\n"\n'
            )
            fake_python.chmod(0o755)
            trace = root / "trace"
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "TRACE": str(trace)})

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            source.write_text("fixed\n")
            self.assertIn("nothing was committed", check().stdout)
            self.assertEqual(test.read_text(), "original test\n")
            git("add", "tornado/web.py")
            git("commit", "-qm", "fix")
            self.assertIn("no committed Tornado test file was changed", check().stdout)
            test.write_text("agent test\n")
            git("add", "tornado/test/web_test.py")
            git("commit", "-qm", "test")
            env["AGENT_HASH"] = git("hash-object", "tornado/test/web_test.py")
            trace.write_text("")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(test.read_text(), "agent test\n")
            entries = [line.split("|", 1) for line in trace.read_text().splitlines()]
            self.assertEqual(len(entries), 2)
            self.assertEqual(entries[0][1], env["AGENT_HASH"])
            self.assertNotEqual(entries[1][1], env["AGENT_HASH"])
            for path, _ in entries:
                self.assertNotEqual(Path(path).resolve(), work.resolve())
                self.assertFalse(Path(path).exists())
            self.assertNotEqual(entries[0][0], entries[1][0])
            self.assertEqual(len(git("worktree", "list", "--porcelain").split("worktree ")) - 1, 1)
            env["FAIL_AGENT"] = "1"
            self.assertIn("tornado.test.web_test failed", check().stdout)
            del env["FAIL_AGENT"]
            self.assertEqual(test.read_text(), "agent test\n")
            (work / "tornado/test/helper.py").write_text("untracked helper\n")
            env["REQUIRE_HELPER"] = "1"
            self.assertNotEqual(check().returncode, 0, "an untracked helper cannot make tests pass")
            del env["REQUIRE_HELPER"]
            fake_python.write_text('#!/bin/sh\nprintf "Ran 0 tests in 0.1s\\nOK\\n"\n')
            self.assertIn("ran no tests", check().stdout)
            fake_python.write_text('#!/bin/sh\nprintf "success without a test count\\n"\n')
            self.assertEqual(check().returncode, 3)
            fake_python.write_text('#!/bin/sh\nprintf "Ran 1 test in 0.1s\\nOK (skipped=1)\\n"\n')
            self.assertIn("skipped all its tests", check().stdout)
            self.assertEqual(check().returncode, 3)
            source.write_text("uncommitted\n")
            self.assertIn("tracked changes were left uncommitted", check().stdout)
            self.assertEqual(test.read_text(), "agent test\n")


if __name__ == "__main__":
    unittest.main()
