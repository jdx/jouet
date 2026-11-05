import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit  # noqa: E402


def run_tests(workdir: Path, module: str) -> None:
    try:
        proc = subprocess.run(
            ["python3", "-m", "tornado.test.runtests", module], cwd=workdir,
            capture_output=True, text=True, timeout=240,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"could not run {module}: {error}")
        return
    expect(proc.returncode == 0, f"{module} failed:\n{proc.stdout}{proc.stderr}")
    if proc.returncode == 0:
        summary = re.search(r"Ran (\d+) tests? in", proc.stdout + proc.stderr)
        if not summary:
            UNVERIFIED.append(f"{module} did not report how many tests ran")
        elif int(summary.group(1)) == 0:
            expect(False, f"{module} ran no tests")
        else:
            skipped = re.search(r"skipped=(\d+)", proc.stdout + proc.stderr)
            if skipped and int(skipped.group(1)) >= int(summary.group(1)):
                UNVERIFIED.append(f"{module} skipped all its tests")


root = Path.cwd()
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
tests = sorted(path for path in committed_files()
               if path.startswith("tornado/test/") and path.endswith("_test.py")
               and Path(path).is_file() and not Path(path).is_symlink()
               and subprocess.run(["git", "cat-file", "-t", f"HEAD:{path}"],
                                  capture_output=True, text=True).stdout.strip() == "blob")
expect(bool(tests), "no committed Tornado test file was changed")
for path in tests:
    run_tests(root, path.removesuffix(".py").replace("/", "."))

# Install the independent fixture in an isolated checkout of the committed
# solution, never over the agent's test file in its retained worktree.
with tempfile.TemporaryDirectory(prefix="tornado-check-") as directory:
    checkout = Path(directory)
    created = subprocess.run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                             capture_output=True, text=True)
    if created.returncode:
        UNVERIFIED.append(f"could not create hidden-test checkout: {created.stderr.strip()}")
    else:
        try:
            target = checkout / "tornado/test/web_test.py"
            if not target.is_file() or target.is_symlink() or not target.resolve().is_relative_to(checkout.resolve()):
                UNVERIFIED.append("hidden-test target is not a regular file in the checkout")
            else:
                shutil.copyfile(Path(__file__).parent / "hidden" / "web_test.py", target)
                run_tests(checkout, "tornado.test.web_test")
        finally:
            removed = subprocess.run(["git", "worktree", "remove", "--force", str(checkout)],
                                     capture_output=True, text=True)
            if removed.returncode:
                UNVERIFIED.append(f"could not remove hidden-test checkout: {removed.stderr.strip()}")
finish()
