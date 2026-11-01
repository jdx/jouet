import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# The fix's own bats tests, which the agent never saw, replace whatever is in
# the work tree: a file argument is read from the directory hk was run in.
# Only the six the fix added run: others in the file need tools, such as
# prettier, that the check has not installed.
shutil.copyfile(Path(__file__).parent / "hidden" / "check.bats", "test/check.bats")
FIX_TESTS = "^check files (resolves|follows|keeps|with a symlinked)"


def run(*cmd: str, env: dict | None = None) -> tuple[bool, str]:
    full = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd(), **(env or {})}
    # hk's test setup writes the global git config of its own HOME, which
    # the check's GIT_CONFIG_GLOBAL=/dev/null would refuse.
    full.pop("GIT_CONFIG_GLOBAL", None)
    proc = subprocess.run(cmd, capture_output=True, text=True, env=full, timeout=1100)
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-3000:]


ok, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    path = f"{os.getcwd()}/target/debug:{os.environ.get('PATH', '')}"
    ok, out = run(
        "test/bats/bin/bats", "--filter", FIX_TESTS, "test/check.bats", env={"PATH": path}
    )
    expect(ok, f"the hidden bats tests fail:\n{out}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
