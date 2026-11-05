import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, head_sha, new_commit  # noqa: E402


def check_ruff(path: str, mode: str, added: bool) -> None:
    """Only count a new failure when the same file passes at the task base."""
    args = ["ruff", "check"] if mode == "lint" else ["ruff", "format", "--check"]
    try:
        current = subprocess.run([*args, "--", path], capture_output=True, text=True, timeout=60)
        if current.returncode == 0:
            return
        if current.returncode != 1:
            UNVERIFIED.append(f"Ruff {mode} could not check {path}: {current.stderr.strip()}")
            return
        if added:
            expect(False, f"Ruff {mode} fails on new file {path}:\n{current.stdout}{current.stderr}")
            return
        original = subprocess.run(["git", "show", f"{head_sha()}:{path}"], capture_output=True, timeout=30)
        if original.returncode:
            UNVERIFIED.append(f"Ruff {mode} cannot compare {path} with its base")
            return
        baseline = subprocess.run(
            [*args, "--stdin-filename", path, "-"], input=original.stdout,
            capture_output=True, timeout=60,
        )
        if baseline.returncode == 0:
            expect(False, f"Ruff {mode} fails on {path}:\n{current.stdout}{current.stderr}")
        else:
            UNVERIFIED.append(f"Ruff {mode} also fails or cannot run on the base version of {path}")
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"Ruff {mode} could not check {path}: {error}")

# Test the public Command API without replacing the agent's tests.
program = r'''
import click

cases = [
    ("Weigh apples vs. pears.", 30, "Weigh apples vs. pears."),
    ("Choose e.g. apples and pears.", 40, "Choose e.g. apples and pears."),
    ("Continue here. another word follows.", 50, "Continue here. another word follows."),
    ("First sentence. Second sentence.", 40, "First sentence."),
    ("Pick a fruit. 3 remain.", 40, "Pick a fruit."),
    ("First line\ncontinues.\n\nSecond paragraph.", 45, "First line continues."),
    ("Weigh apples vs. pears and plums.", 20, "Weigh apples vs...."),
    ("ab cd", 2, "..."),
]
for text, limit, expected in cases:
    actual = click.Command("example", help=text).get_short_help_str(limit)
    if actual != expected:
        raise AssertionError(f"{text!r}: got {actual!r}, expected {expected!r}")
assert click.Command("example", help="Long text", short_help="Exact.").get_short_help_str() == "Exact."
'''
run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                     env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60)
expect(run.returncode == 0, f"short-help cases failed:\n{run.stdout}\n{run.stderr}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
changed = committed_files()
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
