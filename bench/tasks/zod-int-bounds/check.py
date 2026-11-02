import subprocess
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own tests, which the agent never saw, replace whatever is in the
# work tree. The issue asks about numbers; the upstream fix also covers the
# analogous, pre-existing bigint bug, which is reported separately.
tests = "packages/zod/src/v4/classic/tests"
for name in ("bigint.test.ts", "to-json-schema.test.ts"):
    shutil.copyfile(Path(__file__).parent / "hidden" / name, f"{tests}/{name}")

proc = subprocess.run(
    ["node_modules/.bin/vitest", "run", f"{tests}/to-json-schema.test.ts"],
    capture_output=True, text=True, timeout=240,
)
expect(proc.returncode == 0, f"the hidden number tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
try:
    # The task's outer check timeout is 300s. Leave room after the primary
    # 240s check so this diagnostic cannot use up the task's whole budget.
    related = subprocess.run(
        ["node_modules/.bin/vitest", "run", f"{tests}/bigint.test.ts"],
        capture_output=True, text=True, timeout=20,
    )
    print(f"related pre-existing bigint case: {'passed' if related.returncode == 0 else 'failed'}")
except (OSError, subprocess.TimeoutExpired) as error:
    print(f"related pre-existing bigint case: could not check ({error})")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
