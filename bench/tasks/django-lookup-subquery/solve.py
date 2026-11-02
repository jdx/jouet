import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (8914f47), with its tests and release note.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "Fixed #35972 -- Fixed lookup crashes after subquery annotations.",
    "django/contrib/gis/db/models/lookups.py",
    "django/contrib/postgres/search.py",
    "django/db/backends/base/schema.py",
    "django/db/models/expressions.py",
    "django/db/models/functions/comparison.py",
    "django/db/models/lookups.py",
    "docs/ref/models/expressions.txt",
    "docs/releases/6.0.txt",
    "tests/custom_lookups/tests.py",
    "tests/expressions/tests.py",
    "tests/migrations/test_operations.py",
)
