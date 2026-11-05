import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/click-short-help/check.py"


class ClickCheckerTest(unittest.TestCase):
    def test_ruff_failures_are_compared_and_unavailable_checks_are_unverified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            source = work / "source with spaces.py"
            source.write_text("value = 1\n")
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            git("add", source.name)
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            source.write_text("value = 2\n")
            git("add", source.name)
            git("commit", "-qm", "fix")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            (bin_dir / "git").symlink_to(shutil.which("git"))
            python = bin_dir / "python3"
            python.write_text('#!/bin/sh\nexit "${FAKE_BEHAVIOR_EXIT:-0}"\n')
            python.chmod(0o755)
            ruff = bin_dir / "ruff"
            ruff.write_text(
                '#!/bin/sh\n'
                '[ "$1" = check ] && exit 0\n'
                'case "$*" in *--stdin-filename*) exit "${FAKE_BASE_EXIT:-0}";; esac\n'
                'exit "${FAKE_FORMAT_EXIT:-0}"\n'
            )
            ruff.chmod(0o755)
            env.update({"PATH": str(bin_dir), "BENCH_HEAD_SHA": base})

            def check(**extra):
                return subprocess.run([sys.executable, "-I", str(CHECKER)], cwd=work,
                                      env={**env, **extra}, capture_output=True, text=True)

            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = check(FAKE_FORMAT_EXIT="1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("Ruff format fails on source with spaces.py", result.stdout)
            result = check(FAKE_FORMAT_EXIT="1", FAKE_BASE_EXIT="1")
            self.assertEqual(result.returncode, 3)
            self.assertTrue(result.stdout.rstrip().endswith("JOUET_BENCH_UNVERIFIED_V1"))
            result = check(FAKE_FORMAT_EXIT="2")
            self.assertEqual(result.returncode, 3)
            self.assertTrue(result.stdout.rstrip().endswith("JOUET_BENCH_UNVERIFIED_V1"))
            added = work / "new.py"
            added.write_text("value = 3\n")
            git("add", added.name)
            git("commit", "-qm", "add file")
            result = check(FAKE_FORMAT_EXIT="1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("Ruff format fails on new file new.py", result.stdout)
            ruff.unlink()
            result = check()
            self.assertEqual(result.returncode, 3)
            self.assertTrue(result.stdout.rstrip().endswith("JOUET_BENCH_UNVERIFIED_V1"))
            result = check(FAKE_BEHAVIOR_EXIT="1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("short-help cases failed", result.stdout)


if __name__ == "__main__":
    unittest.main()
