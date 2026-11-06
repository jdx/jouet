import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The implementation and tests merged in pallets/click#3021.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("fix: preserve empty prompt suffixes", "src/click/termui.py", "tests/test_utils.py")
