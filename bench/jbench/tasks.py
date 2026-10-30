"""Benchmark tasks: a fixture repository, a prompt and a success check.

A task is a directory under `bench/tasks/`:

- `task.toml`: the prompt, a description, the chore and rules it exercises,
  a timeout, and optionally `branch` (checked out for the agent) and
  `untracked` (files left uncommitted in the work tree, such as another
  session's scratch output);
- `repo/`: the files committed on `main`;
- `pr/` (optional): files committed on `branch` on top of `repo/`;
- `github.json` (optional): the fake GitHub's fixture. `{{head_sha}}` and
  `{{base_sha}}` are replaced with the commits made here, and `{{head_short}}`
  and `{{base_short}}` with their first seven characters;
- `check.py`: run in the work tree after the agent stops. Exit 0 means the
  task succeeded. It gets `BENCH_BASE_SHA`, `BENCH_HEAD_SHA` and, for a task
  with a fake GitHub, `BENCH_GITHUB_STATE` (the fixture after the agent's
  writes);
- `solve.py`: a reference solution, run in a fresh work tree by
  `check-tasks` to prove `check.py` passes for a correct answer (and fails
  without one). It gets `BENCH_GITHUB_URL` for a task with a fake GitHub.

Commits use a fixed author and date, so a task's SHAs are the same in every
run and on every machine.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

TASKS_DIR = Path(__file__).resolve().parent.parent / "tasks"
IGNORE = shutil.ignore_patterns("__pycache__", "*.pyc")

GIT_ENV = {
    "GIT_AUTHOR_NAME": "jouet bench",
    "GIT_AUTHOR_EMAIL": "bench@example.invalid",
    "GIT_COMMITTER_NAME": "jouet bench",
    "GIT_COMMITTER_EMAIL": "bench@example.invalid",
    "GIT_AUTHOR_DATE": "2026-10-01T12:00:00Z",
    "GIT_COMMITTER_DATE": "2026-10-01T12:00:00Z",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
}


@dataclass
class Task:
    name: str
    dir: Path
    description: str
    prompt: str
    chore: str
    rules: list[str]
    timeout_s: int
    smoke: bool = False
    branch: str | None = None
    untracked: dict[str, str] = field(default_factory=dict)
    probes: list[dict[str, Any]] = field(default_factory=list)

    @property
    def has_github(self) -> bool:
        return (self.dir / "github.json").exists()

    def github_fixture(self, shas: dict[str, str]) -> dict[str, Any] | None:
        if not self.has_github:
            return None
        text = (self.dir / "github.json").read_text()
        shas = {**shas, **{k.replace("_sha", "_short"): v[:7] for k, v in shas.items()}}
        for key, value in shas.items():
            text = text.replace("{{" + key + "}}", value)
        return json.loads(text)


def load(name: str) -> Task:
    d = TASKS_DIR / name
    with (d / "task.toml").open("rb") as f:
        raw = tomllib.load(f)
    for required in ("check.py", "solve.py", "repo"):
        if not (d / required).exists():
            raise ValueError(f"task {name} has no {required}")
    return Task(
        name=name,
        dir=d,
        description=raw["description"],
        prompt=raw["prompt"].strip(),
        chore=raw["chore"],
        rules=list(raw.get("rules", [])),
        timeout_s=int(raw.get("timeout_s", 600)),
        smoke=bool(raw.get("smoke", False)),
        branch=raw.get("branch"),
        untracked=dict(raw.get("untracked", {})),
        probes=list(raw.get("probe", [])),
    )


def all_names() -> list[str]:
    return sorted(p.name for p in TASKS_DIR.iterdir() if (p / "task.toml").exists())


def git(workdir: Path, *args: str) -> str:
    env = {**os.environ, **GIT_ENV}
    return subprocess.run(
        ["git", *args], cwd=workdir, env=env, check=True, capture_output=True, text=True
    ).stdout.strip()


def prepare(task: Task, workdir: Path) -> dict[str, str]:
    """Builds the task's work tree in `workdir`; returns its SHAs."""
    shutil.copytree(task.dir / "repo", workdir, ignore=IGNORE)
    git(workdir, "init", "-q", "-b", "main")
    git(workdir, "add", "-A")
    git(workdir, "commit", "-q", "-m", "initial commit")
    base = git(workdir, "rev-parse", "HEAD")
    if task.branch:
        git(workdir, "checkout", "-q", "-b", task.branch)
    if (task.dir / "pr").exists():
        shutil.copytree(task.dir / "pr", workdir, dirs_exist_ok=True, ignore=IGNORE)
        git(workdir, "add", "-A")
        git(workdir, "commit", "-q", "-m", f"work on {task.branch or 'main'}")
    head = git(workdir, "rev-parse", "HEAD")
    # The remote jouet reads the repository from; nothing is ever pushed.
    git(workdir, "remote", "add", "origin", "https://github.com/jdx/demo.git")
    for rel, text in task.untracked.items():
        path = workdir / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return {"base_sha": base, "head_sha": head}


def check_env(shas: dict[str, str], state: Path | None) -> dict[str, str]:
    env = {"BENCH_BASE_SHA": shas["base_sha"], "BENCH_HEAD_SHA": shas["head_sha"]}
    if state is not None:
        env["BENCH_GITHUB_STATE"] = str(state)
    return env


def run_check(task: Task, workdir: Path, env: dict[str, str], timeout_s: int = 120) -> dict[str, Any]:
    """Runs `check.py`; its exit code says whether the task succeeded."""
    full = {**os.environ, **GIT_ENV, **env}
    full.pop("PYTHONPATH", None)
    try:
        proc = subprocess.run(
            ["python3", "-I", str(task.dir / "check.py")],
            cwd=workdir,
            env=full,
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired:
        return {"passed": False, "exit_code": None, "output": "check timed out"}
    output = (proc.stdout + proc.stderr).strip()
    return {"passed": proc.returncode == 0, "exit_code": proc.returncode, "output": output[-4000:]}
