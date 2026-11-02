import subprocess
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own tests, which the agent never saw, replace whatever is in the
# work tree: signed hex and octal integers parse and encode as JSON.
for name in ("json_test.go", "lib_test.go"):
    shutil.copyfile(Path(__file__).parent / "hidden" / name, f"pkg/yqlib/{name}")

# Only the fix's tests: the base already fails TestTomlScenarios.
proc = subprocess.run(
    ["go", "test", "./pkg/yqlib/", "-run", "^(TestJSONScenarios|TestParseInt64)$"],
    capture_output=True, text=True, timeout=540,
)
expect(proc.returncode == 0, f"the hidden tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
