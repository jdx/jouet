import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import (
    UNVERIFIED,
    committed_files,
    dirty,
    expect,
    finish,
    head_sha,
    new_commit,
)


def check_ruff(path: str, mode: str, added: bool) -> None:
    """Only count a new failure when the same file passes at the task base."""
    args = ["ruff", "check", "--no-fix", "--no-fix-only"] if mode == "lint" else ["ruff", "format", "--check"]
    try:
        current = subprocess.run([*args, "--", path], capture_output=True, text=True, timeout=60, check=False)
        if current.returncode == 0:
            return
        if current.returncode != 1:
            UNVERIFIED.append(f"Ruff {mode} could not check {path}: {current.stderr.strip()}")
            return
        if added:
            expect(False, f"Ruff {mode} fails on new file {path}:\n{current.stdout}{current.stderr}")
            return
        original = subprocess.run(["git", "show", f"{head_sha()}:{path}"], capture_output=True, timeout=30, check=False)
        if original.returncode:
            UNVERIFIED.append(f"Ruff {mode} cannot compare {path} with its base")
            return
        baseline = subprocess.run(
            [*args, "--stdin-filename", path, "-"], input=original.stdout,
            capture_output=True, timeout=60, check=False,
        )
        if baseline.returncode == 0:
            expect(False, f"Ruff {mode} fails on {path}:\n{current.stdout}{current.stderr}")
        else:
            UNVERIFIED.append(f"Ruff {mode} also fails or cannot run on the base version of {path}")
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"Ruff {mode} could not check {path}: {error}")

# Test the public option API without replacing the agent's tests.
program = r'''
import click

class StrictDefault:
    def __eq__(self, other):
        if isinstance(other, str):
            raise ValueError("cannot compare this default to a string")
        return NotImplemented

    def __str__(self):
        return "strict-default"

context = click.Context(click.Command("example"))
for default, expected in ((StrictDefault(), "strict-default"), ("", '""'), ("value", "value")):
    option = click.Option(["--example"], default=default, show_default=True)
    record = option.get_help_record(context)
    assert record is not None and f"[default: {expected}]" in record[1], record
'''
run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                     env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60, check=False)
expect(run.returncode == 0, f"option default cases failed:\n{run.stdout}\n{run.stderr}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
changed = committed_files()
tests = sorted(path for path in changed if path.startswith("tests/")
               and Path(path).name.startswith("test_") and path.endswith(".py")
               and Path(path).is_file() and not Path(path).is_symlink()
               and Path(path).resolve().is_relative_to(Path.cwd().resolve()))
expect(bool(tests), "no committed Python test file was changed")
if tests:
    try:
        tested = subprocess.run(["python3", "-m", "pytest", "-q", "--", *tests],
                                capture_output=True, text=True, timeout=90, check=False)
        output = tested.stdout + tested.stderr
        if tested.returncode == 1 and re.search(r"\b[1-9][0-9]* failed\b", output):
            expect(False, f"committed tests failed:\n{output}")
        elif tested.returncode == 5:
            expect(False, f"committed test files collected no tests:\n{output}")
        elif tested.returncode:
            UNVERIFIED.append(f"committed tests could not report a test result:\n{output}")
        elif not re.search(r"\b[1-9][0-9]* passed\b", output) or re.search(r"\b[1-9][0-9]* skipped\b", output):
            UNVERIFIED.append(f"committed tests did not report complete passing coverage:\n{output}")
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"committed tests could not run: {error}")
added_result = subprocess.run(
    ["git", "diff", "--name-only", "--diff-filter=A", head_sha(), "HEAD"],
    capture_output=True, text=True, check=True,
)
added = set(added_result.stdout.splitlines())
for path in sorted(changed):
    if path.endswith(".py") and Path(path).is_file():
        check_ruff(path, "lint", path in added)
        check_ruff(path, "format", path in added)
finish()
