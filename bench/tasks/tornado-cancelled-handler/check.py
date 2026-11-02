import subprocess
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own web tests, which the agent never saw, replace whatever is in
# the work tree: a handler that raises CancelledError gets a 500, not a hang.
shutil.copyfile(Path(__file__).parent / "hidden" / "web_test.py", "tornado/test/web_test.py")

# tornado's own runner: plain unittest fails its tests on the access log.
proc = subprocess.run(
    ["python3", "-m", "tornado.test.runtests", "tornado.test.web_test"],
    capture_output=True, text=True, timeout=240,
)
expect(proc.returncode == 0, f"the hidden web tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
