"""Check that fnox's editor regression suite actually ran completely."""

import os
import re
import subprocess
import sys
from pathlib import Path

from _check import UNVERIFIED, expect, head_sha


def failed_added(output: str, added: list[str]) -> bool:
    results = re.findall(r"^not ok (\d+)\b([^\n]*)$", output, re.MULTILINE)
    return any(
        result[1].strip() in added and not re.search(r"#\s*(skip|todo)\b", result[1], re.IGNORECASE)
        for result in results
    )


def inspect_tap(output: str, exit_code: int, added: list[str] | None = None) -> str | None:
    """Return why a Bats run does not prove complete coverage, if anything."""
    if added and failed_added(output, added):
        return "a committed regression test failed"
    plans = re.findall(r"^1\.\.(\d+)\s*$", output, re.MULTILINE)
    results = re.findall(r"^(not ok|ok) (\d+)\b([^\n]*)$", output, re.MULTILINE)
    if len(plans) != 1 or int(plans[0]) == 0:
        return "missing or empty Bats test plan"
    count = int(plans[0])
    if len(results) != count or [int(result[1]) for result in results] != list(range(1, count + 1)):
        return "incomplete or inconsistent Bats test results"
    if any(re.search(r"#\s*(skip|todo)\b", result[2], re.IGNORECASE) for result in results):
        return "Bats tests were skipped or marked TODO"
    names = [result[2].strip() for result in results]
    if added and any(names.count(name) != 1 for name in added):
        return "a committed regression test was not reported exactly once"
    if exit_code != 0 or any(result[0] == "not ok" for result in results):
        return "Bats suite failed"
    return None


def check_bats() -> None:
    changed = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=AM", head_sha(), "HEAD", "--", "test/edit.bats"],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()
    if not changed:
        UNVERIFIED.append("no committed editor regression test was changed")
        return
    def names_at(revision: str) -> list[str]:
        contents = subprocess.run(
            ["git", "show", f"{revision}:test/edit.bats"],
            capture_output=True, text=True, check=True,
        ).stdout
        return re.findall(r'^\s*@test\s+"([^"]+)"\s*\{', contents, re.MULTILINE)

    try:
        original = names_at(head_sha())
        current = names_at("HEAD")
    except subprocess.CalledProcessError:
        UNVERIFIED.append("could not compare committed editor tests with the starting revision")
        return
    if (not original or len(set(original)) != len(original) or len(set(current)) != len(current)
            or not set(original).issubset(current)):
        UNVERIFIED.append("starting editor tests are missing, renamed, or ambiguous")
        return
    added = [name for name in current if name not in set(original)]
    if len(added) < 2:
        UNVERIFIED.append("expected two distinct new editor regression tests in standard Bats form")
        return
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard"],
        capture_output=True, text=True, check=True,
    ).stdout
    if untracked:
        UNVERIFIED.append("untracked files could affect the editor tests")
        return
    env = {**os.environ, "PATH": f"{Path.cwd() / 'target/debug'}{os.pathsep}{os.environ.get('PATH', '')}"}
    try:
        run = subprocess.run(
            ["test/bats/bin/bats", "--tap", "test/edit.bats"],
            capture_output=True, text=True, timeout=180, check=False, env=env,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
        stderr = error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else error.stderr or ""
        print(stdout, end="")
        if stderr:
            print(stderr, end="", file=sys.stderr)
        if failed_added(stdout, added):
            expect(False, "a committed regression test failed before the Bats suite timed out")
        else:
            UNVERIFIED.append(f"editor Bats suite timed out: {error}")
        return
    except OSError as error:
        UNVERIFIED.append(f"could not complete the editor Bats suite: {error}")
        return
    print(run.stdout, end="")
    if run.stderr:
        print(run.stderr, end="", file=sys.stderr)
    reason = inspect_tap(run.stdout, run.returncode, added)
    if reason == "a committed regression test failed":
        expect(False, reason)
    elif reason:
        UNVERIFIED.append(reason)
