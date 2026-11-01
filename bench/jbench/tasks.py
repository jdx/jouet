"""Benchmark tasks: a fixture repository, a prompt and a success check.

A task is a directory under `bench/tasks/`:

- `task.toml`: the prompt, a description, the chore and rules it exercises,
  a timeout, and optionally `branch` (checked out for the agent) and
  `untracked` (files left uncommitted in the work tree, such as another
  session's scratch output);
- `repo/`: the files committed on `main`, or instead `[source]` in
  `task.toml`: `git` (a local repository, read only) and `commit`, whose tree
  is committed on `main` as the one commit, without its history;
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

A task on a real project also sets, in `task.toml`:

- `opt_in = true`, so that it runs only when named with `--tasks`;
- `remote = false`, for a repository with no forge remote, and
  `upstream = true` to give it a local bare `origin` instead, holding the
  base commit as `main`, which `main` tracks: an agent that commits on
  `main` still has a branch to measure its change from, as in a clone;
- `check_timeout_s`, for a check that builds the project;
- `[tools]`: programs linked into the agent's `bin/` (name = path), and
  `[env]`: variables set for the agent, such as a toolchain's home. A leading
  `~/` is the user's home, and `{workdir}` is the work tree;
- `path`: directories appended to the agent's `PATH`, after the tools.

Commits use a fixed author and date, so a task's SHAs are the same in every
run and on every machine.
"""

from __future__ import annotations

import hashlib
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


class EnvironmentSetupError(RuntimeError):
    """A task's preflight failed before an agent started."""


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
    source: dict[str, str] | None = None
    opt_in: bool = False
    remote: bool = True
    upstream: bool = False
    check_timeout_s: int = 120
    tools: dict[str, str] = field(default_factory=dict)
    env: dict[str, str] = field(default_factory=dict)
    path: list[str] = field(default_factory=list)
    # Commands timed once for the cost note runs start with (see costs.py).
    cost_seed: list[str] = field(default_factory=list)
    # A fully pinned pip requirements file, relative to the task directory.
    python_requirements: str | None = None

    def agent_env(self, env: dict[str, str], bin_dir: Path, workdir: Path) -> None:
        """Adds the task's tools, variables and PATH entries to a run's environment."""
        for name, target in self.tools.items():
            link = bin_dir / name
            if not link.exists():
                link.symlink_to(expand(target, workdir))
        env.update({k: expand(v, workdir) for k, v in self.env.items()})
        if self.path:
            env["PATH"] = os.pathsep.join([env["PATH"], *(expand(p, workdir) for p in self.path)])

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
    for required in ("check.py", "solve.py") if "source" in raw else ("check.py", "solve.py", "repo"):
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
        source=raw.get("source"),
        opt_in=bool(raw.get("opt_in", False)),
        remote=bool(raw.get("remote", True)),
        upstream=bool(raw.get("upstream", False)),
        check_timeout_s=int(raw.get("check_timeout_s", 120)),
        tools=dict(raw.get("tools", {})),
        env=dict(raw.get("env", {})),
        path=list(raw.get("path", [])),
        cost_seed=list(raw.get("cost", {}).get("seed", [])),
        python_requirements=raw.get("python_requirements"),
    )


def requirements_file(task: Task) -> Path | None:
    if not task.python_requirements:
        return None
    requirements = (task.dir / task.python_requirements).resolve()
    if not requirements.is_relative_to(task.dir.resolve()) or not requirements.is_file():
        raise EnvironmentSetupError(f"{task.name}: Python requirements must be a file inside the task directory")
    return requirements


def setup_environment(task: Task, env: dict[str, str], workdir: Path, cache: Path, log: Path) -> dict[str, str] | None:
    """Create an isolated Python environment before an agent or check starts."""
    requirements = requirements_file(task)
    if requirements is None:
        return None
    # Keep the environment out of the checkout even when .venv is not ignored.
    venv = Path(env["HOME"]) / ".venv"
    if venv.exists():
        raise EnvironmentSetupError(f"{task.name}: refusing to replace an existing .venv")
    python = shutil.which("python3", path=env["PATH"])
    if not python:
        raise EnvironmentSetupError(f"{task.name}: Python 3 is required for environment setup")
    setup_env = {**env, "PIP_CONFIG_FILE": os.devnull, "PIP_CACHE_DIR": str(cache),
                 "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1"}
    bin_dir = venv / ("Scripts" if os.name == "nt" else "bin")
    venv_python = bin_dir / ("python.exe" if os.name == "nt" else "python")
    commands = [
        [python, "-m", "venv", str(venv)],
        [str(venv_python), "-m", "pip", "install", "--require-hashes", "--requirement", str(requirements)],
    ]
    try:
        with log.open("w") as output:
            for command in commands:
                proc = subprocess.run(command, cwd=workdir, env=setup_env, stdout=output, stderr=subprocess.STDOUT,
                                      timeout=300)
                if proc.returncode:
                    raise EnvironmentSetupError(f"{task.name}: Python environment setup failed (log: {log})")
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EnvironmentSetupError(f"{task.name}: Python environment setup failed: {error} (log: {log})") from error
    env["VIRTUAL_ENV"] = str(venv)
    env["PATH"] = os.pathsep.join((str(bin_dir), env["PATH"]))
    try:
        version = subprocess.run([str(venv_python), "--version"], env=env, capture_output=True, text=True, check=True, timeout=30)
    except (OSError, subprocess.SubprocessError) as error:
        raise EnvironmentSetupError(f"{task.name}: cannot read the Python version: {error}") from error
    return {"python": version.stdout.strip(), "requirements_sha256": hashlib.sha256(requirements.read_bytes()).hexdigest()}


def expand(value: str, workdir: Path) -> str:
    if value.startswith("~/"):
        value = str(Path.home() / value[2:])
    return value.replace("{workdir}", str(workdir))


def all_names() -> list[str]:
    return sorted(p.name for p in TASKS_DIR.iterdir() if (p / "task.toml").exists())


def git(workdir: Path, *args: str) -> str:
    env = {**os.environ, **GIT_ENV}
    return subprocess.run(
        ["git", *args], cwd=workdir, env=env, check=True, capture_output=True, text=True
    ).stdout.strip()


def prepare(task: Task, workdir: Path) -> dict[str, str]:
    """Builds the task's work tree in `workdir`; returns its SHAs."""
    if task.source:
        copy_source(task.source, workdir)
    else:
        shutil.copytree(task.dir / "repo", workdir, ignore=IGNORE)
        git(workdir, "init", "-q", "-b", "main")
        git(workdir, "add", "-A")
    git(workdir, "commit", "-q", "-m", "initial commit")
    # An archived tree contains gitlinks, not the files they point to. Populate
    # them at the recorded revisions so an agent can run the project's tests.
    if task.source and (workdir / ".gitmodules").exists():
        git(workdir, "submodule", "update", "--init", "--recursive")
    base = git(workdir, "rev-parse", "HEAD")
    if task.branch:
        git(workdir, "checkout", "-q", "-b", task.branch)
    if (task.dir / "pr").exists():
        shutil.copytree(task.dir / "pr", workdir, dirs_exist_ok=True, ignore=IGNORE)
        git(workdir, "add", "-A")
        git(workdir, "commit", "-q", "-m", f"work on {task.branch or 'main'}")
    head = git(workdir, "rev-parse", "HEAD")
    if task.remote:
        # The remote jouet reads the repository from; nothing is ever pushed.
        git(workdir, "remote", "add", "origin", "https://github.com/jdx/demo.git")
        if task.upstream:
            # The refs a clone has, with no server to fetch them from: what
            # the agent measures its change from.
            branch = task.branch or "main"
            git(workdir, "update-ref", "refs/remotes/origin/main", base)
            git(workdir, "update-ref", f"refs/remotes/origin/{branch}", head)
            git(workdir, "branch", "-q", f"--set-upstream-to=origin/{branch}", branch)
            git(workdir, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main")
    elif task.upstream:
        origin = workdir.parent / "origin.git"
        git(workdir.parent, "init", "-q", "--bare", "-b", "main", str(origin))
        git(workdir, "remote", "add", "origin", str(origin))
        git(workdir, "push", "-q", "origin", f"{base}:refs/heads/main")
        git(workdir, "fetch", "-q", "origin")
        git(workdir, "branch", "-q", "--set-upstream-to=origin/main", "main")
        git(workdir, "remote", "set-head", "origin", "main")
    for rel, text in task.untracked.items():
        path = workdir / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return {"base_sha": base, "head_sha": head}


def copy_source(source: dict[str, str], workdir: Path) -> None:
    """Stages the tree of `source`'s commit in a new repository at `workdir`.

    Every tracked file is staged by name, with `-f`, because a project can
    track files its own `.gitignore` matches, which `git add -A` would drop."""
    repo = Path(source["git"]).expanduser()
    commit = source["commit"]
    workdir.mkdir(parents=True)
    archive = subprocess.run(["git", "-C", str(repo), "archive", commit], check=True, capture_output=True)
    subprocess.run(["tar", "-x", "-C", str(workdir)], input=archive.stdout, check=True)
    git(workdir, "init", "-q", "-b", "main")
    entries = subprocess.run(
        ["git", "-C", str(repo), "ls-tree", "-r", "-z", commit], check=True, capture_output=True
    ).stdout.split(b"\0")
    files, links = [], []
    for entry in filter(None, entries):
        metadata, name = entry.split(b"\t", 1)
        mode, kind, oid = metadata.split(b" ")
        if kind == b"commit":
            links.append(mode + b" " + oid + b"\t" + name + b"\0")
        else:
            files.append(name + b"\0")
    env = {**os.environ, **GIT_ENV}
    subprocess.run(
        ["git", "add", "-f", "--pathspec-from-file=-", "--pathspec-file-nul"],
        cwd=workdir,
        env=env,
        input=b"".join(files),
        check=True,
        capture_output=True,
    )
    if links:
        subprocess.run(
            ["git", "update-index", "-z", "--index-info"], cwd=workdir, env=env,
            input=b"".join(links), check=True, capture_output=True,
        )
    tree = git(workdir, "write-tree")
    want = git(repo, "rev-parse", f"{commit}^{{tree}}")
    if tree != want:
        raise RuntimeError(f"the staged tree {tree} is not {commit}'s tree {want}")


def check_env(shas: dict[str, str], state: Path | None) -> dict[str, str]:
    env = {"BENCH_BASE_SHA": shas["base_sha"], "BENCH_HEAD_SHA": shas["head_sha"]}
    if state is not None:
        env["BENCH_GITHUB_STATE"] = str(state)
    return env


def run_check(task: Task, workdir: Path, base: dict[str, str], env: dict[str, str]) -> dict[str, Any]:
    """Runs `check.py` in `base`, the run's own environment, so that what it
    builds and runs stays in the run's directory as the agent's did; its
    exit code says whether the task succeeded."""
    timeout_s = task.check_timeout_s
    full = {**base, **GIT_ENV, **env}
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
