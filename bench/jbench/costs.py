"""The timings a task's runs start with, as CI would have kept them.

`[cost] seed` in task.toml lists the commands to time. They run once through
`jouet test --cmd`, in a work tree of their own rather than a run's, whose
build they would warm. The note jouet keeps for them under
`refs/notes/jouet-cost` is cached by the task's source and commands, and by
the machine, since the timings are this machine's. Every run, in both arms,
starts with that note on its HEAD, so the jouet arm's primer can name the
slow checks, as it would in a repository whose CI keeps them.
"""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import socket
import subprocess
import tempfile
from pathlib import Path
from typing import Any, Callable

from . import arms as arms_mod
from . import tasks as tasks_mod

NOTES_REF = "jouet-cost"


def cache_path(task: tasks_mod.Task) -> Path:
    data = [task.name, task.source, task.cost_seed, socket.gethostname()]
    if task.python_requirements:
        data.append(hashlib.sha256((task.dir / task.python_requirements).read_bytes()).hexdigest())
    key = json.dumps(data)
    digest = hashlib.sha256(key.encode()).hexdigest()[:16]
    root = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    return root / "jouet-bench" / "cost-seeds" / f"{task.name}-{digest}.txt"


def seed(task: tasks_mod.Task, jouet_bin: Path, work_root: Path, log: Callable[[str], None]) -> str | None:
    """The task's cost note: from the cache, else timed now. None when the
    task seeds nothing, or when no command was timed."""
    if not task.cost_seed:
        return None
    cache = cache_path(task)
    if cache.exists():
        return cache.read_text()
    log(f"timing {len(task.cost_seed)} commands for {task.name}'s cost note (once; cached at {cache})")
    run_dir = work_root / f"cost-seed-{task.name}"
    workdir = run_dir / "work"
    tasks_mod.prepare(task, workdir)
    # TMPDIR in /tmp, as a run's is: a project's tests run below it with an
    # environment of their own, without the run's mise ceiling, and below
    # the user's home they would read and write the user's mise config.
    tmp = Path(tempfile.mkdtemp(prefix="jb-seed-", dir="/tmp"))
    env = arms_mod.base_env(run_dir, None, "jdx/demo", tmp)
    arms_mod.wrapper(run_dir / "bin" / "jouet", jouet_bin.resolve(), {})
    task.agent_env(env, run_dir / "bin", workdir)
    try:
        tasks_mod.setup_environment(task, env, workdir, work_root / "python-cache", run_dir / "environment.log")
    except tasks_mod.EnvironmentSetupError:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    for command in task.cost_seed:
        proc = subprocess.run(
            ["jouet", "test", "--cmd", "--", *shlex.split(command)],
            cwd=workdir,
            env=env,
            capture_output=True,
            text=True,
            timeout=task.check_timeout_s,
        )
        log(f"    {command}: exit {proc.returncode}, {(proc.stdout.strip().splitlines() or [''])[-1]}")
    shutil.rmtree(tmp, ignore_errors=True)
    proc = subprocess.run(
        ["git", "notes", "--ref", NOTES_REF, "show", "HEAD"], cwd=workdir, env=env, capture_output=True, text=True
    )
    if proc.returncode != 0 or not proc.stdout.strip():
        log(f"    no timings were kept for {task.name}; its runs start without a cost note")
        return None
    cache.parent.mkdir(parents=True, exist_ok=True)
    cache.write_text(proc.stdout)
    return proc.stdout


def apply(workdir: Path, note: str, commit: str) -> None:
    """Puts the cost note on `commit` in a run's work tree."""
    tasks_mod.git(workdir, "notes", "--ref", NOTES_REF, "add", "-f", "-m", note.strip(), commit)


def seeds(planned: list[Any], jouet_bin: Path, work_root: Path, log: Callable[[str], None]) -> dict[str, str]:
    """Each planned task's cost note, by task name."""
    notes = {}
    for name in sorted({p.task for p in planned}):
        note = seed(tasks_mod.load(name), jouet_bin, work_root, log)
        if note:
            notes[name] = note
    return notes
