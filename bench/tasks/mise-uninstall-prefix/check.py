import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own e2e test, which the agent never saw, replaces whatever is in
# the work tree. It covers the issue (1.1.1 is not 1.1.12), `latest` (only
# the newest version goes) and a backend's own matching (java's `+build`).
shutil.copyfile(Path(__file__).parent / "hidden" / "test_uninstall", "e2e/cli/test_uninstall")


def run(*cmd: str) -> tuple[bool, str]:
    env = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd()}
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=1500)
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-3000:]


ok, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    ok, out = run("mise", "run", "test:e2e", "test_uninstall")
    expect(ok, f"the hidden e2e test fails:\n{out}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
