import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

CHECKER = Path(__file__).resolve().parents[1] / "tasks/hk-check-file-args/check.py"


class HkCheckerTest(unittest.TestCase):
    def test_requires_a_clean_committed_checkout_before_running_checks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            bindir = root / "bin"
            bindir.mkdir()
            cargo = bindir / "cargo"
            cargo.write_text("#!/bin/sh\nexit 0\n")
            cargo.chmod(0o755)
            bats = work / "test/bats/bin/bats"
            bats.parent.mkdir(parents=True)
            bats.write_text("#!/bin/sh\nexit 0\n")
            bats.chmod(0o755)
            (work / "test/check.bats").write_text("original\n")
            source = work / "source"
            source.write_text("original\n")
            env = {**os.environ, "PATH": f"{bindir}{os.pathsep}{os.environ['PATH']}",
                   "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            git("add", "source", "test")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")

            def check():
                return subprocess.run([sys.executable, "-I", str(CHECKER)], cwd=work,
                                      env=env, capture_output=True, text=True, check=False)

            source.write_text("uncommitted fix\n")
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("nothing was committed", result.stdout)
            self.assertIn("uncommitted changes", result.stdout)
            self.assertEqual((work / "test/check.bats").read_text(), "original\n")

            git("add", "source")
            git("commit", "-qm", "fix")
            (work / "extra").write_text("untracked\n")
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("uncommitted changes", result.stdout)
            (work / "extra").unlink()

            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("ok", result.stdout)


if __name__ == "__main__":
    unittest.main()
