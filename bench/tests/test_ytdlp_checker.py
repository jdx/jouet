import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/ytdlp-metadata-field/check.py"


class YtdlpCheckerTest(unittest.TestCase):
    def test_requires_clean_commit_with_a_present_test_file(self):
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
            (work / "test").mkdir()
            git("add", "source.py")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            python = bin_dir / "python3"
            python.write_text('#!/bin/sh\nprintf \'["3", "first\\\\nsecond", "Name", "Song"]\\n\'\n')
            python.chmod(0o755)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            (work / "source.py").write_text("after\n")
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("nothing was committed", result.stdout)
            git("add", "source.py")
            git("commit", "-qm", "fix")
            test = work / "test/test_metadata.py"
            test.write_text("def test_metadata(): pass\n")
            self.assertIn("no committed test file", check().stdout)
            git("add", "test/test_metadata.py")
            git("commit", "-qm", "test")
            self.assertEqual(check().returncode, 0)
            (work / "source.py").write_text("uncommitted\n")
            self.assertIn("tracked changes were left uncommitted", check().stdout)
            git("restore", "source.py")
            test.unlink()
            git("add", "test/test_metadata.py")
            git("commit", "-qm", "remove test")
            test.write_text("def test_metadata(): pass\n")
            self.assertIn("no committed test file", check().stdout)


if __name__ == "__main__":
    unittest.main()
