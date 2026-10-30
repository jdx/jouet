"""Helpers for the tasks' `check.py` and `solve.py` scripts.

A check runs in the task's work tree after the agent stops, with
`BENCH_BASE_SHA` (the commit on `main`), `BENCH_HEAD_SHA` (the commit the
agent started from) and, for a task with a fake GitHub, `BENCH_GITHUB_STATE`.
It prints what it found and exits 0 only when the task was done.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

PROBLEMS: list[str] = []


def git(*args: str) -> str:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout.strip()


def expect(ok: bool, problem: str) -> None:
    if not ok:
        PROBLEMS.append(problem)


def finish() -> None:
    for p in PROBLEMS:
        print(f"- {p}")
    if PROBLEMS:
        sys.exit(1)
    print("ok")


def head_sha() -> str:
    return os.environ["BENCH_HEAD_SHA"]


def tests_pass(*args: str, env: dict[str, str] | None = None) -> tuple[bool, str]:
    proc = subprocess.run(
        [sys.executable, "-m", "unittest", *args],
        capture_output=True,
        text=True,
        env={**os.environ, **(env or {})},
        timeout=120,
    )
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-2000:]


def unchanged(*paths: str) -> bool:
    """The paths are as the agent found them, in the work tree and in HEAD."""
    started = head_sha()
    worktree = subprocess.run(["git", "diff", "--quiet", started, "--", *paths]).returncode == 0
    staged = subprocess.run(["git", "diff", "--quiet", "--cached", started, "--", *paths]).returncode == 0
    committed = subprocess.run(["git", "diff", "--quiet", started, "HEAD", "--", *paths]).returncode == 0
    return worktree and staged and committed


def committed_files() -> set[str]:
    """Files changed by the commits the agent made."""
    return set(filter(None, git("diff", "--name-only", head_sha(), "HEAD").splitlines()))


def new_commit() -> bool:
    return git("rev-parse", "HEAD") != head_sha()


def dirty(*paths: str) -> list[str]:
    """Tracked files with changes that were not committed."""
    out = git("status", "--porcelain", "--untracked-files=no", "--", *paths)
    return [line[3:] for line in out.splitlines()]


def github_state() -> dict[str, Any]:
    return json.loads(Path(os.environ["BENCH_GITHUB_STATE"]).read_text())


def run_python(code: str, env: dict[str, str] | None = None) -> tuple[bool, str]:
    proc = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True,
        text=True,
        env={**os.environ, **(env or {})},
        timeout=60,
    )
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-1500:]


def commit(message: str, *paths: str) -> None:
    """For a `solve.py`: commits exactly `paths`."""
    subprocess.run(["git", "add", "--", *paths], check=True)
    subprocess.run(["git", "commit", "-q", "-m", message, "--", *paths], check=True)
