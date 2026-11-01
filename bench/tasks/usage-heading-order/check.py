import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own conformance tests, which the agent never saw, replace
# whatever is in the work tree: the ungrouped section comes after the named
# ones when its commands sort after theirs, and the built-in `help` last.
shutil.copyfile(
    Path(__file__).parent / "hidden" / "heading_help.rs", "conformance/tests/heading_help.rs"
)


def run(*cmd: str) -> tuple[bool, str]:
    env = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd()}
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=1100)
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-3000:]


ok, out = run("cargo", "test", "-p", "usage-conformance", "--test", "heading_help")
expect(ok, f"the hidden conformance tests fail:\n{out}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
