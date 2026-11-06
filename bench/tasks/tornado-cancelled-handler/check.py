import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit

DEADLINE = time.monotonic() + 180

def run_tests(workdir: Path, module: str) -> None:
    timeout = min(240, DEADLINE - time.monotonic())
    if timeout <= 0:
        UNVERIFIED.append(f"time budget exhausted before {module}")
        return
    try:
        proc = subprocess.run(
            ["python3", "-m", "tornado.test.runtests", module], cwd=workdir,
            capture_output=True, text=True, timeout=timeout, check=False,
        )
    except subprocess.TimeoutExpired as error:
        for stream in (error.stdout, error.stderr):
            if stream:
                text = stream.decode(errors="replace") if isinstance(stream, bytes) else stream
                print(text, end="" if text.endswith("\n") else "\n")
        UNVERIFIED.append(f"could not run {module}: {error}")
        return
    except OSError as error:
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


def check_committed(hidden: bool, tests: list[str]) -> None:
    """Run each phase in a fresh checkout, isolated from untracked helpers."""
    with tempfile.TemporaryDirectory(prefix="tornado-check-") as directory:
        checkout = Path(directory)
        remaining = DEADLINE - time.monotonic()
        if remaining <= 0:
            UNVERIFIED.append("time budget exhausted before creating the test checkout")
            return
        try:
            created = subprocess.run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                                     capture_output=True, text=True, timeout=min(15, remaining), check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            UNVERIFIED.append(f"could not create test checkout: {error}")
            return
        if created.returncode:
            UNVERIFIED.append(f"could not create test checkout: {created.stderr.strip()}")
            return
        try:
            if hidden:
                target = checkout / "tornado/test/web_test.py"
                if not target.exists() and not target.is_symlink():
                    expect(False, "the hidden-test target was removed from the committed tree")
                elif not target.is_file() or target.is_symlink() or not target.resolve().is_relative_to(checkout.resolve()):
                    UNVERIFIED.append("hidden-test target is not a regular file in the checkout")
                else:
                    shutil.copyfile(Path(__file__).parent / "hidden" / "web_test.py", target)
                    run_tests(checkout, "tornado.test.web_test")
            else:
                for path in tests:
                    run_tests(checkout, path.removesuffix(".py").replace("/", "."))
        finally:
            try:
                removed = subprocess.run(["git", "worktree", "remove", "--force", str(checkout)],
                                         capture_output=True, text=True, timeout=30, check=False)
                if removed.returncode:
                    UNVERIFIED.append(f"could not remove test checkout: {removed.stderr.strip()}")
            except (OSError, subprocess.TimeoutExpired) as error:
                UNVERIFIED.append(f"could not remove test checkout: {error}")


def main() -> None:
    expect(new_commit(), "nothing was committed")
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    tests = sorted(path for path in committed_files()
                   if path.startswith("tornado/test/") and path.endswith("_test.py")
                   and Path(path).is_file() and not Path(path).is_symlink()
                   and subprocess.run(["git", "cat-file", "-t", f"HEAD:{path}"],
                                      capture_output=True, text=True, check=False).stdout.strip() == "blob")
    expect(bool(tests), "no committed Tornado test file was changed")
    if tests:
        check_committed(hidden=False, tests=tests)
    check_committed(hidden=True, tests=tests)
    finish()


if __name__ == "__main__":
    main()
