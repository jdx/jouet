"""The `jouet-init` arm's starting point: the project as the jouet-init skill
leaves it, as a user who ran `jouet setup` and then `/jouet-init` has it.

The skill runs once per task, starting commit, jouet build, harness and
model, in an isolated home like any run. What it changed in the work tree
is kept as a patch, with what the run cost, under
`~/.cache/jouet-bench/init/`. Every `jouet-init` run applies that patch and
commits it before the agent starts, so its own work begins from a clean
tree that already holds the configuration. The skill's run is not
counted: a project commits the configuration once and keeps it.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
from dataclasses import asdict
from pathlib import Path
from typing import Any

from . import arms as arms_mod
from . import credentials
from . import tasks as tasks_mod
from .harness import Harness

PROMPT = "Use the jouet-init skill to configure jouet for this project."
CACHE = Path.home() / ".cache" / "jouet-bench" / "init"
TIMEOUT_S = 1800


def key(task: tasks_mod.Task, head_sha: str, jouet_bin: Path, harness: Harness, opts: dict[str, Any]) -> str:
    """A hash only: like a measured run's directory, the skill's paths carry
    no task name, which a project's tests could see."""
    digest = hashlib.sha256(jouet_bin.read_bytes()).hexdigest()[:16]
    model = opts.get(f"{harness.name}_model") or ""
    raw = f"{task.name}\0{head_sha}\0{digest}\0{harness.name}\0{model}"
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
    task.agent_env(env, run_dir / "bin", workdir)
    tasks_mod.setup_environment(task, env, workdir, work_root / "python-cache", run_dir / "environment.log")
    arms_mod.setup_jouet(harness, env, workdir)
    arms_mod.login_shell_path(env)
    config_dir = Path(env[harness.config_dir_env])
    lent = credentials.lend(arms_mod.credential_files(harness), config_dir)
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
        )
    finally:
        credentials.give_back(lent)
    usage = harness.usage(agent["lines"], config_dir)
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


def trust(env: dict[str, str], workdir: Path) -> None:
    """Trusts the project's jouet config, as the user does once after
    jouet-init: each run's home is new, so the trust is not there yet. A
    trust that fails would measure an untrusted config, so it stops the run."""
    proc = subprocess.run(["jouet", "trust", "--yes"], cwd=workdir, env=env, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"jouet trust --yes failed: {proc.stderr.strip() or proc.stdout.strip()}")
