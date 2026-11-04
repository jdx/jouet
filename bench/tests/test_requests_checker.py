import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "tasks/requests-proxied-files/check.py"


class RequestsCheckerTest(unittest.TestCase):
    def test_requires_a_committed_test_file_that_still_exists(self):
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
            (work / "tests").mkdir()
            (work / "tests/test_existing.py").write_text("def test_existing(): pass\n")
            git("add", "source.py", "tests/test_existing.py")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (work / "source.py").write_text("after\n")
            git("add", "source.py")
            git("commit", "-qm", "fix")

            # The independent multipart cases have their own validation. Here
            # they succeed so this test isolates the committed-test requirement.
            bin_dir = root / "bin"
            bin_dir.mkdir()
            fake_python = bin_dir / "python3"
            fake_python.write_text("#!/bin/sh\nexit 0\n")
            fake_python.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_HEAD_SHA": base})

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("no committed test file was changed", result.stdout)
            existing = work / "tests/test_existing.py"
            existing.unlink()
            git("add", "tests/test_existing.py")
            git("commit", "-qm", "delete test")
            existing.write_text("def test_existing(): pass\n")
            self.assertNotEqual(check().returncode, 0, "an untracked replacement is not committed")
            existing.unlink()
            git("checkout", base, "--", "tests/test_existing.py")
            git("commit", "-qm", "restore test")
            added = work / "tests/test_upload.py"
            added.write_text("def test_upload(): pass\n")
            self.assertNotEqual(check().returncode, 0, "an untracked test is not committed")
            git("add", "tests/test_upload.py")
            git("commit", "-qm", "add test")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            added.unlink()
            git("add", "tests/test_upload.py")
            git("commit", "-qm", "remove test")
            self.assertNotEqual(check().returncode, 0, "a deleted test is not evidence")
            (work / "tests/test_existing.py").write_text("def test_existing(): assert True\n")
            git("add", "tests/test_existing.py")
            git("commit", "-qm", "extend test")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
