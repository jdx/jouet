import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own tests, which the agent never saw, replace whatever is in the
# work tree: the documented lookup pattern against a Subquery and a Func, every
# CharField lookup against a Subquery, and a db_default function in a migration.
hidden = Path(__file__).parent / "hidden"
for name, dest in (
    ("custom_lookups_tests.py", "tests/custom_lookups/tests.py"),
    ("expressions_tests.py", "tests/expressions/tests.py"),
    ("migrations_test_operations.py", "tests/migrations/test_operations.py"),
):
    shutil.copyfile(hidden / name, dest)

# Django's own runner, on SQLite. The check's environment drops PYTHONPATH,
# so the work tree is put back on it.
proc = subprocess.run(
    ["python3", "tests/runtests.py", "custom_lookups", "expressions", "migrations.test_operations"],
    capture_output=True, text=True, timeout=240, env={**os.environ, "PYTHONPATH": os.getcwd()},
)
expect(proc.returncode == 0, f"the hidden tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
