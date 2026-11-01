import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (jdx/mise#15116), with its test.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("fix(uninstall): don't remove 3.4.12 when asked to uninstall 3.4.1", "src/cli/uninstall.rs", "e2e/cli/test_uninstall")
