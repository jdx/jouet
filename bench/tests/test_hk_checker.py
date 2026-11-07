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
            bats.write_text("#!/bin/sh\n"
                            "if [ \"$1\" = --tap ] && ! grep -q regression \"$2\"; then exit 7; fi\n"
                            "case $BATS_CASE in\n"
                            "  skip) printf '1..1\\nok 1 regression # skip unavailable\\n' ;;\n"
                            "  fail) printf '1..1\\nnot ok 1 regression\\n'; exit 1 ;;\n"
                            "  *) printf '1..1\\nok 1 regression\\n' ;;\n"
                            "esac\n")
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
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn("no committed Bats regression", result.stdout)
            git("restore", "test/check.bats")
            (work / "test/check.bats").write_text('@test "regression" {\n  true\n}\n')
            git("add", "test/check.bats")
            git("commit", "-qm", "regression")

            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("ok", result.stdout)

            git("restore", "test/check.bats")
            env["BATS_CASE"] = "skip"
            result = check()
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn("unverified", result.stdout)

            git("restore", "test/check.bats")
            env["BATS_CASE"] = "fail"
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("committed Bats regression failed", result.stdout)

            git("restore", "test/check.bats")
            git("rm", "test/check.bats")
            (work / "test/new.bats").write_text('@test "regression" {\n  true\n}\n')
            git("add", "test/new.bats")
            git("commit", "-qm", "replace test file")
            env["BATS_CASE"] = ""
            result = check()
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn("removed or renamed", result.stdout)


if __name__ == "__main__":
    unittest.main()
