import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/usage-heading-order/check.py"


class UsageCheckerTest(unittest.TestCase):
    def test_requires_a_commit_and_no_tracked_changes_before_hidden_test(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            tests = work / "conformance/tests"
            tests.mkdir(parents=True)
            (tests / "heading_help.rs").write_text("original test\n")
            source = work / "source.rs"
            source.write_text("before\n")
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            git("add", "source.rs", "conformance/tests/heading_help.rs")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            cargo = bin_dir / "cargo"
            cargo.write_text("#!/bin/sh\nexit 0\n")
            cargo.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_HEAD_SHA": base})

            def check():
                return subprocess.run([sys.executable, "-I", str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("nothing was committed", result.stdout)
            git("restore", "conformance/tests/heading_help.rs")
            source.write_text("fixed\n")
            git("add", "source.rs")
            git("commit", "-qm", "fix")
            source.write_text("uncommitted changes\n")
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("tracked changes remain uncommitted", result.stdout)
            git("restore", "source.rs", "conformance/tests/heading_help.rs")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
