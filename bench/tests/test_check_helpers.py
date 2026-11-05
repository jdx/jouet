import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


TASKS = Path(__file__).resolve().parents[1] / "tasks"


class CheckHelpersTest(unittest.TestCase):
    def test_dirty_keeps_the_first_character_of_unstaged_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid",
                   "PYTHONPATH": str(TASKS)}

            def git(*args):
                subprocess.run(["git", *args], cwd=work, env=env, check=True, capture_output=True)

            git("init", "-q")
            (work / "README.md").write_text("before\n")
            (work / "second.py").write_text("before\n")
            git("add", "README.md", "second.py")
            git("commit", "-qm", "base")
            (work / "README.md").write_text("after\n")
            (work / "second.py").write_text("after\n")
            git("add", "second.py")
            result = subprocess.run(
                [sys.executable, "-c", "import json; from _check import dirty; print(json.dumps(dirty()))"],
                cwd=work, env=env, check=True, capture_output=True, text=True,
            )
            self.assertEqual(json.loads(result.stdout), ["README.md", "second.py"])


if __name__ == "__main__":
    unittest.main()
