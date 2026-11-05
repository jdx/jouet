import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit  # noqa: E402

# Check the agent's tree before placing the hidden test in it.
committed = new_commit()
expect(committed, "nothing was committed")
expect(not dirty(), "tracked changes remain uncommitted")

# The fix's own conformance tests, which the agent never saw, replace
# whatever is in the work tree: the ungrouped section comes after the named
# ones when its commands sort after theirs, and the built-in `help` last.
# This checks behavior independently; it does not verify the agent's own test.
shutil.copyfile(
    Path(__file__).parent / "hidden" / "heading_help.rs", "conformance/tests/heading_help.rs"
)


def run(*cmd: str) -> tuple[bool, str]:
    env = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd()}
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env, timeout=1100)
    output = proc.stdout
    if len(output) > 8000:
        output = f"... {len(output) - 8000} earlier characters omitted\n" + output[-8000:]
    return proc.returncode == 0, output


ok, out = run("cargo", "test", "-p", "usage-conformance", "--test", "heading_help")
expect(ok, f"the hidden conformance tests fail:\n{out}")
print(f"committed: {committed}")
finish()
