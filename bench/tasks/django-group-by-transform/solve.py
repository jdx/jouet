import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (543e17c), with its test and release note.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "Fixed #36292 -- Fixed crash when aggregating over a group mixing transforms and references.",
    "django/db/models/sql/query.py",
    "docs/releases/5.2.1.txt",
    "tests/aggregation/tests.py",
)
