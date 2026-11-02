import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (colinhacks/zod#6553), with its tests.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(v4): don't let format checks overwrite tighter min/max bounds",
    "packages/zod/src/v4/core/checks.ts",
    "packages/zod/src/v4/classic/tests/bigint.test.ts",
    "packages/zod/src/v4/classic/tests/to-json-schema.test.ts",
)
