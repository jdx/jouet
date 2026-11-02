import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The implementation and tests merged in pallets/click#3865.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("fix: preserve abbreviations in short help",
       "src/click/utils.py", "tests/test_utils/test_make_default_short_help.py")
