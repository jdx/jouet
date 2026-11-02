import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("fix: preserve completion arguments", "completions.go", "completions_test.go")
