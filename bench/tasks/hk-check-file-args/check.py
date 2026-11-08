import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit
from bats_check import check_bats

# Validate the agent's checkout before the checker overwrites its test file.
committed = new_commit()
expect(committed, "nothing was committed")
status = subprocess.run(
    ["git", "status", "--porcelain", "--untracked-files=all"],
    capture_output=True, text=True, check=False,
)
expect(status.returncode == 0 and not status.stdout, "the checkout has uncommitted changes or cannot be inspected")
if not committed or status.returncode != 0 or status.stdout:
    finish()

# The fix's own bats tests, which the agent never saw, replace whatever is in
# the work tree: a file argument is read from the directory hk was run in.
# Only the six the fix added run: others in the file need tools, such as
# prettier, that the check has not installed.
FIX_TESTS = "^check files (resolves|follows|keeps|with a symlinked)"


def run(*cmd: str, env: dict | None = None) -> tuple[bool, str]:
    full = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd(), **(env or {})}
    # hk's test setup writes the global git config of its own HOME, which
    # the check's GIT_CONFIG_GLOBAL=/dev/null would refuse.
    full.pop("GIT_CONFIG_GLOBAL", None)
    proc = subprocess.run(cmd, capture_output=True, text=True, env=full, timeout=1100, check=False)
    return proc.returncode == 0, proc.stdout + proc.stderr


ok, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    check_bats()
    shutil.copyfile(Path(__file__).parent / "hidden" / "check.bats", "test/check.bats")
    path = f"{os.getcwd()}/target/debug:{os.environ.get('PATH', '')}"
    ok, out = run(
        "test/bats/bin/bats", "--filter", FIX_TESTS, "test/check.bats", env={"PATH": path}
    )
    expect(ok, f"the hidden bats tests fail:\n{out}")
finish()
