"""The `jouet-init` arm's starting point: the project as the jouet-init skill
leaves it, as a user who ran `jouet setup` and then `/jouet-init` has it.

The skill runs once per cache identity, in an isolated home like any run.
The identity covers the task definition and path, pinned requirements, starting
commit, jouet build, harness executable, model, effort and initialization prompt.
What it changed in the work tree
is kept as a patch, with what the run cost, under
`~/.cache/jouet-bench/init/`. Every `jouet-init` run applies that patch and
commits it before the agent starts, so its own work begins from a clean
tree that already holds the configuration. The skill's run is not
counted: a project commits the configuration once and keeps it.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
from dataclasses import asdict
from pathlib import Path
from typing import Any

from . import arms as arms_mod
from . import codex_hooks
from . import credentials
from . import tasks as tasks_mod
from .harness import Harness

PROMPT = (
    "Use the jouet-init skill to configure jouet for this project. "
    "You are authorized to create and update the project configuration and documentation files "
    "needed for this setup, including .jouet files and mise.toml."
)
CACHE = Path.home() / ".cache" / "jouet-bench" / "init"
TIMEOUT_S = 1800


class DiskGuard(RuntimeError):
    """Initialization stopped because the agent reached the disk reserve."""


def key(task: tasks_mod.Task, head_sha: str, jouet_bin: Path, harness: Harness, opts: dict[str, Any]) -> str:
    """A hash only: like a measured run's directory, the skill's paths carry
    no task name, which a project's tests could see."""
    requirements = tasks_mod.requirements_file(task)
    task_data = asdict(task)
    task_data["dir"] = str(task.dir.resolve())
    binary = opts.get("harness_bins", {}).get(harness.name)
    harness_binary = None
    if binary:
        path = Path(binary).resolve()
        try:
            stat = path.stat()
            harness_binary = [str(path), stat.st_size, stat.st_mtime_ns]
        except OSError:
            harness_binary = [str(path), None, None]
    data = {
        "schema": 2,
        "task": task_data,
        "requirements_sha256": tasks_mod.requirements_digest(task, requirements) if requirements else None,
        "head_sha": head_sha,
        "jouet_sha256": hashlib.sha256(jouet_bin.read_bytes()).hexdigest(),
        "harness": harness.name,
        "harness_binary": harness_binary,
        "model": opts.get(f"{harness.name}_model"),
        "effort": opts.get("effort"),
        "prompt": PROMPT,
    }
    raw = json.dumps(data, sort_keys=True)
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def git(workdir: Path, *args: str) -> str:
    """git with the bench's own identity and none of the host's config, as
    every other git call of the harness. The output is kept whole: a patch
    needs its last newline."""
    return subprocess.run(
        ["git", *args],
        cwd=workdir,
        env=tasks_mod.git_env(),
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def untracked(workdir: Path) -> list[str]:
    return [p for p in git(workdir, "ls-files", "--others", "--exclude-standard", "-z").split("\0") if p]


def prepared(
    task: tasks_mod.Task,
    harness: Harness,
    shas: dict[str, str],
    opts: dict[str, Any],
    work_root: Path,
    run_agent: Any,
) -> dict[str, Any]:
    """The cached init for this task, made by running the skill when there
    is none: `{"patch": Path, "meta": {...}, "cached": bool}`."""
    jouet_bin = Path(opts["jouet_bin"])
    name = key(task, shas["head_sha"], jouet_bin, harness, opts)
    directory = CACHE / name
    patch, meta_path = directory / "init.patch", directory / "meta.json"
    if patch.exists() and meta_path.exists():
        return {"patch": patch, "meta": json.loads(meta_path.read_text()), "cached": True}

    run_dir = work_root / f"init-{name}"
    workdir = run_dir / "work"
    fresh = tasks_mod.prepare(task, workdir)
    # A task's own untracked files (another session's, say) are not the
    # skill's work, and every run's tree has them already.
    before = set(untracked(workdir))
    env = arms_mod.build(
        run_dir,
        harness,
        arms_mod.ARMS["jouet"],
        opts["harness_bins"][harness.name],
        jouet_bin,
        None,
        "jdx/demo",
        Path(opts["tmp_root"]) / f"init-{name}",
    )
    codex_home = env.get("CODEX_HOME")
    codex_tools = arms_mod.codex_tools(run_dir / "bin", True) if harness.name == "codex" else None
    if harness.name == "codex" and arms_mod.CODEX_ISOLATION_ENV.intersection(task.env):
        raise codex_hooks.HookPreflightError("A task changed a Codex isolation path")
    task.agent_env(env, run_dir / "bin", workdir)
    if codex_tools is not None and arms_mod.codex_tools(run_dir / "bin", True) != codex_tools:
        raise codex_hooks.HookPreflightError("A task replaced a Codex benchmark tool")
    if harness.name == "codex" and (env.get("CODEX_HOME") != codex_home or
                                    arms_mod.CODEX_ISOLATION_ENV.intersection(opts.get("jouet_env") or {})):
        raise codex_hooks.HookPreflightError("A task or override changed CODEX_HOME")
    tasks_mod.setup_environment(task, env, workdir, work_root / "python-cache", run_dir / "environment.log")
    if codex_tools is not None:
        if arms_mod.codex_tools(run_dir / "bin", True) != codex_tools:
            raise codex_hooks.HookPreflightError("Setup replaced a Codex benchmark tool")
        arms_mod.verify_codex_path(run_dir / "bin", env, True)
    # setup installs the skill only when asked for.
    arms_mod.setup_jouet(harness, env, workdir, ("--init-skill",))
    arms_mod.reviewed_codex(harness, env, workdir, opts)
    # The skill sets the project up in mise, and checks what it wrote by
    # running it: as in the person's own repository, mise may run it.
    had_mise = has_mise(workdir)
    mise_env(env, workdir, created=not had_mise)
    arms_mod.login_shell_path(env)
    config_dir = Path(env[harness.config_dir_env])
    try:
        lent = credentials.lend(arms_mod.credential_files(harness), config_dir, exclusive=harness.name == "codex")
    except FileExistsError as error:
        raise codex_hooks.HookPreflightError("Dedicated Codex credentials appeared during setup") from error
    try:
        agent = run_agent(
            harness,
            harness.command(harness.binary, PROMPT, opts, workdir),
            env,
            workdir,
            config_dir,
            TIMEOUT_S,
            opts.get("max_tokens_per_run"),
            run_dir / "agent.jsonl",
            opts.get("min_free_bytes", 0),
            opts.get("disk_guard_event"),
        )
    finally:
        if harness.name == "codex":
            credentials.give_back_and_remove(lent)
        else:
            credentials.give_back(lent)
    if agent["stopped"] == "disk_guard":
        raise DiskGuard("disk reserve reached during project initialization")
    usage = harness.usage(agent["lines"], config_dir)
    if harness.name == "codex" and not arms_mod.codex_started(env):
        raise codex_hooks.HookPreflightError(f"Codex did not record the jouet session-start hook; see {run_dir / 'agent.jsonl'}")
    # A run that failed configured nothing to keep: say so, and cache none.
    if agent["exit_code"] != 0 or agent["stopped"] or usage.is_error:
        raise RuntimeError(
            f"jouet-init for {task.name} did not finish (exit {agent['exit_code']}, "
            f"stopped {agent['stopped']}); see {run_dir / 'agent.jsonl'}"
        )
    # Everything it left, committed or not, against where it started.
    git(workdir, "add", "-A")
    # Left out by path, so they stay out even if the skill committed them.
    paths = ["--", ".", *(f":(exclude,literal){p}" for p in sorted(before))]
    diff = git(workdir, "diff", "--cached", "--binary", fresh["head_sha"], *paths)
    files = [f for f in git(workdir, "diff", "--cached", "--name-only", fresh["head_sha"], *paths).splitlines() if f]
    directory.mkdir(parents=True, exist_ok=True)
    patch.write_text(diff)
    meta = {
        "task": task.name,
        "head_sha": fresh["head_sha"],
        "harness": harness.name,
        "exit_code": agent["exit_code"],
        "stopped": agent["stopped"],
        "wall_s": agent["wall_s"],
        "usage": asdict(usage),
        "files": files,
        "mise_created": not had_mise and has_mise(workdir),
    }
    meta_path.write_text(json.dumps(meta, indent=2))
    return {"patch": patch, "meta": meta, "cached": False}


def apply(workdir: Path, patch: Path, task: tasks_mod.Task, shas: dict[str, str]) -> None:
    """Put initialization on the baseline, then replay any task commits.

    The patch is made against the task head. A three-way apply also lets it
    apply to the baseline when the task already has commits; if it cannot,
    fail the trial rather than measure configuration as task work.
    """
    if not patch.read_text().strip():
        return
    base = shas["base_sha"]
    with tempfile.TemporaryDirectory(prefix="init-base-", dir=workdir.parent) as directory:
        baseline = Path(directory)
        git(workdir, "worktree", "add", "--detach", "-q", str(baseline), base)
        try:
            try:
                git(baseline, "apply", "--3way", "--index", "--binary", str(patch))
            except subprocess.CalledProcessError as e:
                raise RuntimeError(f"init patch cannot be applied to the task baseline: {e.stderr.strip()}") from e
            git(baseline, "commit", "-q", "--no-verify", "-m", "chore: configure jouet (jouet-init)")
            configured_base = git(baseline, "rev-parse", "HEAD").strip()
        finally:
            git(workdir, "worktree", "remove", "--force", str(baseline))
    try:
        git(workdir, "rebase", "--onto", configured_base, base)
    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"task commits cannot be replayed on the initialized baseline: {e.stderr.strip()}") from e
    shas["base_sha"] = configured_base
    shas["head_sha"] = git(workdir, "rev-parse", "HEAD").strip()
    if task.upstream:
        if not task.remote:
            # Never contact the configured URL: this is the per-run local origin.
            origin = workdir.parent / "origin.git"
            git(workdir, "push", "-q", str(origin), f"{configured_base}:refs/heads/main")
        git(workdir, "update-ref", "refs/remotes/origin/main", configured_base)
        if task.remote and task.branch and task.branch != "main":
            git(workdir, "update-ref", f"refs/remotes/origin/{task.branch}", shas["head_sha"])


# Where mise looks for a project's shared config, as jouet-init may write it.
MISE_CONFIGS = ("mise.toml", ".mise.toml", "mise/config.toml", ".mise/config.toml",
                ".config/mise.toml", ".config/mise/config.toml")

# Tools a mise.toml that jouet-init created pins, installed once and shared
# by every run, as on a developer's machine, rather than in each run's home.
MISE_DATA = Path.home() / ".cache" / "jouet-bench" / "mise-data"


def has_mise(workdir: Path) -> bool:
    return any((workdir / name).is_file() for name in MISE_CONFIGS)


def mise_env(env: dict[str, str], workdir: Path, created: bool) -> None:
    """Lets mise run the work tree's config, as a person who reviewed it
    trusts it, and, when jouet-init created that config, keeps the tools it
    pins in the shared data directory unless the task names its own."""
    trusted = [p for p in env.get("MISE_TRUSTED_CONFIG_PATHS", "").split(os.pathsep) if p]
    if str(workdir) not in trusted:
        env["MISE_TRUSTED_CONFIG_PATHS"] = os.pathsep.join([*trusted, str(workdir)])
    if created:
        env.setdefault("MISE_DATA_DIR", str(MISE_DATA))


def trust(env: dict[str, str], workdir: Path) -> None:
    """Trusts the project's jouet config, as the user does once after
    jouet-init: each run's home is new, so the trust is not there yet. A
    trust that fails would measure an untrusted config, so it stops the run."""
    proc = subprocess.run(["jouet", "trust", "--yes"], cwd=workdir, env=env, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"jouet trust --yes failed: {proc.stderr.strip() or proc.stdout.strip()}")
