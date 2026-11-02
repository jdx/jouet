import subprocess
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own tests, which the agent never saw, replace whatever is in the
# work tree: configureOutput() after copyInheritedSettings() leaves the
# original command's settings alone.
name = "tests/command.configureOutput.test.js"
shutil.copyfile(Path(__file__).parent / "hidden" / Path(name).name, name)

proc = subprocess.run(["node_modules/.bin/jest", name], capture_output=True, text=True, timeout=240)
expect(proc.returncode == 0, f"the hidden tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
