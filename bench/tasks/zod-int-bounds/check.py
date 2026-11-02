import subprocess
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own tests, which the agent never saw, replace whatever is in the
# work tree: a format check after min/max keeps the tighter bounds.
tests = "packages/zod/src/v4/classic/tests"
for name in ("bigint.test.ts", "to-json-schema.test.ts"):
    shutil.copyfile(Path(__file__).parent / "hidden" / name, f"{tests}/{name}")

proc = subprocess.run(
    ["node_modules/.bin/vitest", "run", f"{tests}/bigint.test.ts", f"{tests}/to-json-schema.test.ts"],
    capture_output=True, text=True, timeout=240,
)
expect(proc.returncode == 0, f"the hidden tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
