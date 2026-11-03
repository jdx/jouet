import os
from pathlib import Path
import subprocess
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/actual-running-balance/check.py"


class ActualCheckerTest(unittest.TestCase):
    def test_requires_the_working_tree_used_by_the_check_to_be_committed(self):
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
            (work / "base").write_text("base\n")
            git("add", "base")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (work / "packages/desktop-client/e2e").mkdir(parents=True)
            (work / "regression.test.js").write_text("test\n")
            (work / "upcoming-release-notes").mkdir()
            (work / "upcoming-release-notes/fix.md").write_text("note\n")
            git("add", "regression.test.js", "upcoming-release-notes/fix.md")
            git("commit", "-qm", "fix")

            # Stand in for Playwright and release-note validation: this test
            # exercises the checker's commit requirement, not those tools.
            bin_dir = root / "bin"
            bin_dir.mkdir()
            node = bin_dir / "node"
            node.write_text("#!/bin/sh\nmkdir -p blob-report\ntouch blob-report/report.zip\nexit 0\n")
            node.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_BASE_SHA": base})

            def check():
                return subprocess.run(["python3", str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            (work / "uncommitted-source.js").write_text("required by the fix\n")
            self.assertNotEqual(check().returncode, 0)
            git("add", "uncommitted-source.js")
            git("commit", "-qm", "include source")
            result = check()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((work / "blob-report/report.zip").exists())
