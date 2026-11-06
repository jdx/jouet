import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/cobra-completion-args/check.py"


class CobraCheckerTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("gofmt"), "gofmt is not installed")
    def test_changed_go_files_are_compared_with_the_base(self):
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
            source = work / "file with spaces.go"
            source.write_text("package main\n\nvar x = 1\n")
            git("add", source.name)
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            source.write_text("package main\n\nvar x = 2\n")
            git("add", source.name)
            git("commit", "-qm", "change")
            bindir = root / "bin"
            bindir.mkdir()
            (bindir / "git").symlink_to(shutil.which("git"))
            (bindir / "gofmt").symlink_to(shutil.which("gofmt"))
            go = bindir / "go"
            go.write_text("#!/bin/sh\nexit 0\n")
            go.chmod(0o755)
            env.update({"PATH": str(bindir), "BENCH_HEAD_SHA": base})

            def check():
                return subprocess.run([sys.executable, "-I", str(CHECKER)], cwd=work,
                                      env=env, capture_output=True, text=True)

            self.assertEqual(check().returncode, 0)
            source.write_text("package main\nvar x=2\n")
            git("add", source.name)
            git("commit", "-qm", "unformatted")
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("gofmt fails on file with spaces.go", result.stdout)
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            source.write_text("package main\nvar x=3\n")
            git("add", source.name)
            git("commit", "-qm", "already unformatted")
            result = check()
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn("JOUET_BENCH_UNVERIFIED_V1", result.stdout)
            source.write_text("package main\n\nvar x = 3\n")
            added = work / "new.go"
            added.write_text("package main\nvar y=1\n")
            git("add", source.name, added.name)
            git("commit", "-qm", "add file")
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("gofmt fails on new file new.go", result.stdout)
            (bindir / "gofmt").unlink()
            result = check()
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn("JOUET_BENCH_UNVERIFIED_V1", result.stdout)


if __name__ == "__main__":
    unittest.main()
