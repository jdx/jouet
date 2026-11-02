import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own aggregation tests, which the agent never saw, replace whatever
# is in the work tree: values() over a transform and a column, then Count.
shutil.copyfile(Path(__file__).parent / "hidden" / "aggregation_tests.py", "tests/aggregation/tests.py")

# Django's own runner, on SQLite, with the neighbouring GROUP BY tests, which
# a fix must not break. The check's environment drops PYTHONPATH, so the
# work tree is put back on it.
proc = subprocess.run(
    ["python3", "tests/runtests.py", "aggregation", "aggregation_regress"],
    capture_output=True, text=True, timeout=240, env={**os.environ, "PYTHONPATH": os.getcwd()},
)
expect(proc.returncode == 0, f"the hidden tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
