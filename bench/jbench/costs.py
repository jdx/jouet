"""The timings a task's runs start with, as CI would have kept them.

`[cost] seed` in task.toml lists the commands to time. They run once through
`jouet test --cmd`, in a work tree of their own rather than a run's, whose
build they would warm. The note jouet keeps for them under
`refs/notes/jouet-cost` is cached by the source, setup, tools, harness code,
jouet binary, commands and machine. Every run, in both arms,
starts with that note on its HEAD, so the jouet arm's primer can name the
slow checks, as it would in a repository whose CI keeps them.
"""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import signal
import socket
import subprocess
import tempfile
from collections import Counter
from collections.abc import Callable
from pathlib import Path
from typing import Any

from . import arms as arms_mod
from . import tasks as tasks_mod

NOTES_REF = "jouet-cost"


def note_at_head(workdir: Path, env: dict[str, str]) -> str:
    proc = subprocess.run(["git", "notes", "--ref", NOTES_REF, "show", "HEAD"],
                          cwd=workdir, env=env, capture_output=True, text=True, check=False)
    return proc.stdout if proc.returncode == 0 else ""


def timed_check(argv: list[str], workdir: Path, env: dict[str, str], timeout: int) -> subprocess.CompletedProcess:
    """Stop the check and its children before reporting a timed-out seed."""
    proc = subprocess.Popen(argv, cwd=workdir, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, start_new_session=os.name == "posix")
    try:
        stdout, stderr = proc.communicate(timeout=timeout)
    except BaseException:
        if os.name == "posix":
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        else:
            try:
                subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                               capture_output=True, timeout=10, check=False)
            except (OSError, subprocess.TimeoutExpired):
                pass
            if proc.poll() is None:
                proc.kill()
        proc.communicate()
        raise
    return subprocess.CompletedProcess(argv, proc.returncode, stdout, stderr)


def harness_hashes() -> list[str]:
    """Code that constructs and seeds the isolated environment."""
    return [hashlib.sha256(Path(path).read_bytes()).hexdigest()
            for path in (__file__, arms_mod.__file__, tasks_mod.__file__)]


def cache_path(task: tasks_mod.Task, jouet_bin: Path) -> Path:
    # Setup and the tools used by it can change what a check costs even when
    # the source commit and check command stay the same.
    requirements = tasks_mod.requirements_file(task)
    requirements_hash = tasks_mod.requirements_digest(task, requirements) if requirements else None
    programs = {"node", "python3"}
    programs.update(shlex.split(command)[0] for command in (*task.setup, *task.cost_seed) if shlex.split(command))
    programs.update(task.tools.values())
    tools = {}
    for program in sorted(programs):
        if "/" in program and not program.startswith("~/") and not Path(program).is_absolute():
            # It will be resolved in the task's work tree, not this process's
            # cwd. The source revision and setup are already part of the key.
            tools[program] = ["worktree", program]
            continue
        found = Path(program).expanduser() if "/" in program else arms_mod.resolve(program)
        if found:
            path = found.resolve()
            try:
                stat = path.stat()
                tools[program] = [str(path), stat.st_size, stat.st_mtime_ns]
            except OSError:
                tools[program] = None
        else:
            tools[program] = None
    with jouet_bin.open("rb") as binary:
        jouet_hash = hashlib.file_digest(binary, "sha256").hexdigest()
    data = [task.name, task.source, task.cost_seed, task.setup, task.tools, task.env, task.path,
            tools, jouet_hash, harness_hashes(), socket.gethostname()]
    if requirements_hash:
        data.append(requirements_hash)
    key = json.dumps(data, sort_keys=True)
    digest = hashlib.sha256(key.encode()).hexdigest()[:16]
    root = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    return root / "jouet-bench" / "cost-seeds" / f"{task.name}-{digest}.txt"


def seed(task: tasks_mod.Task, jouet_bin: Path, work_root: Path, log: Callable[[str], None],
         log_dir: Path | None = None) -> str | None:
    """The task's cost note: from the cache, else timed now. None when the
    task seeds nothing, or when no command was timed."""
    if not task.cost_seed:
        return None
    cache = cache_path(task, jouet_bin)
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
    setup_log = (log_dir or run_dir) / f"{task.name}.environment.log"
    if task.python_requirements:
        setup_log.parent.mkdir(parents=True, exist_ok=True)
    try:
        tasks_mod.setup_environment(task, env, workdir, work_root / "python-cache", setup_log)
        note = note_at_head(workdir, env)
        incomplete = False
        for command in task.cost_seed:
            try:
                proc = timed_check(
                    ["jouet", "test", "--cmd", "--", *shlex.split(command)],
                    workdir, env, task.check_timeout_s,
                )
            except (OSError, subprocess.TimeoutExpired) as error:
                log(f"    {command} could not be timed: {error}; the cost seed is incomplete and will not be cached")
                incomplete = True
                break
            log(f"    {command}: exit {proc.returncode}, {(proc.stdout.strip().splitlines() or [''])[-1]}")
            if proc.returncode != 0:
                log(f"    {command} failed; the cost seed is incomplete and will not be cached")
                incomplete = True
            updated = note_at_head(workdir, env)
            if not (Counter(updated.splitlines()) - Counter(note.splitlines())):
                log(f"    no timing was recorded for {command}; the cost seed is incomplete and will not be cached")
                incomplete = True
            note = updated
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    if incomplete:
        log(f"    {task.name}: runs start without a cost note because the seed is incomplete")
        return None
    if not note.strip():
        log(f"    no timings were kept for {task.name}; its runs start without a cost note")
        return None
    cache.parent.mkdir(parents=True, exist_ok=True)
    cache.write_text(note)
    return note


def apply(workdir: Path, note: str, commit: str) -> None:
    """Puts the cost note on `commit` in a run's work tree."""
    tasks_mod.git(workdir, "notes", "--ref", NOTES_REF, "add", "-f", "-m", note.strip(), commit)


def seeds(planned: list[Any], jouet_bin: Path, work_root: Path, log: Callable[[str], None],
          log_dir: Path | None = None) -> dict[str, str]:
    """Each planned task's cost note, by task name."""
    notes = {}
    for name in sorted({p.task for p in planned}):
        try:
            note = seed(tasks_mod.load(name), jouet_bin, work_root, log, log_dir)
        except tasks_mod.EnvironmentSetupError as error:
            log(f"    {name}: cost seed setup failed ({error}); trials will retry setup")
            continue
        if note:
            notes[name] = note
    return notes
