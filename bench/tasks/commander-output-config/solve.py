import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (tj/commander.js#2350), with its test.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "Make .configureOutput() create copy of settings instead of in-place change",
    "lib/command.js",
    "tests/command.configureOutput.test.js",
)
