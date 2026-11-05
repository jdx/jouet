import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/tornado-cancelled-handler/check.py"


class TornadoCheckerTest(unittest.TestCase):
    def test_preserves_agent_tests_and_requires_committed_changes(self):
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
                '#!/bin/sh\nprintf "%s\\n" "$PWD" >> "$TRACE"\n'
                'if [ "${FAIL_AGENT:-}" = 1 ] && [ "$PWD" = "$WORK" ]; then exit 1; fi\n'
                'printf "Ran 1 test in 0.1s\\nOK\\n"\n'
            )
            fake_python.chmod(0o755)
            trace = root / "trace"
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "TRACE": str(trace),
                        "WORK": str(work.resolve())})

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
            trace.write_text("")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(test.read_text(), "agent test\n")
            paths = trace.read_text().splitlines()
            self.assertEqual(len(paths), 2)
            self.assertEqual(Path(paths[0]).resolve(), work.resolve())
            self.assertNotEqual(Path(paths[1]).resolve(), work.resolve())
            self.assertFalse(Path(paths[1]).exists())
            self.assertEqual(len(git("worktree", "list", "--porcelain").split("worktree ")) - 1, 1)
            env["FAIL_AGENT"] = "1"
            self.assertIn("tornado.test.web_test failed", check().stdout)
            del env["FAIL_AGENT"]
            self.assertEqual(test.read_text(), "agent test\n")
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
