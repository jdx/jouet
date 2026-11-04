"""Benchmark tasks: a fixture repository, a prompt and a success check.

A task is a directory under `bench/tasks/`, or under a directory named by
`--tasks-dir` or the `JOUET_BENCH_TASKS` path list, such as tasks on private
repositories that cannot live in this one. A task's name is its directory's
name, and must be unique across all of them:

- `task.toml`: the prompt, a description, the chore and rules it exercises,
  a timeout, and optionally `branch` (checked out for the agent) and
  `untracked` (files left uncommitted in the work tree, such as another
  session's scratch output);
- `repo/`: the files committed on `main`, or instead `[source]` in
  `task.toml`: `git` (a local repository, read only) and `commit`, whose tree
  is committed on `main` as the one commit, without its history by default;
  `history = true` preserves the pinned commit and its ancestors instead;
- `pr/` (optional): files committed on `branch` on top of `repo/`;
- `github.json` (optional): the fake GitHub's fixture. `{{head_sha}}` and
  `{{base_sha}}` are replaced with the commits made here, and `{{head_short}}`
  and `{{base_short}}` with their first seven characters;
- `check.py`: run in the work tree after the agent stops. Exit 0 means the
  task succeeded. It gets `BENCH_BASE_SHA`, `BENCH_HEAD_SHA`,
  `BENCH_CHECK_HELPERS` (the directory holding `_check.py`, for a task outside
  `bench/tasks/`) and, for a task with a fake GitHub, `BENCH_GITHUB_STATE`
  (the fixture after the agent's writes);
- `solve.py`: a reference solution, run in a fresh work tree by
  `check-tasks` to prove `check.py` passes for a correct answer (and fails
  without one). It gets `BENCH_GITHUB_URL` for a task with a fake GitHub.

A task on a real project also sets, in `task.toml`:

- `opt_in = true`, so that it runs only when named with `--tasks`;
- `remote = false`, for a repository with no forge remote, and
  `upstream = true` to give it a local bare `origin` instead, holding the
  base commit as `main`, which `main` tracks: an agent that commits on
  `main` still has a branch to measure its change from, as in a clone.
  `upstream_url` makes that `origin`'s URL, as a clone's would be, for a
  project whose code or tests parse it; git's `insteadOf` still sends every
  fetch and push to the local repository. Use a host that cannot resolve,
  such as `https://git.example.invalid/owner/repo.git`;
- `check_timeout_s`, for a check that builds the project;
- `[tools]`: programs linked into the agent's `bin/` (name = path, which
  replaces a default tool of that name, or a bare program name, found on the
  host's `PATH` as the default tools are), and
  `[env]`: variables set for the agent, such as a toolchain's home. A leading
  `~/` is the user's home, and `{workdir}` is the work tree;
- `path`: directories appended to the agent's `PATH`, after the tools;
- `setup`: commands run in the work tree, with the host's environment (and
  `~/` and `{workdir}` expanded in their arguments), after
  it is committed and before the agent's clock starts, such as an offline
  `npm ci` from the user's cache. What they write must be ignored by git.

Commits use a fixed author and date, so a task's SHAs are the same in every
run and on every machine.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

TASKS_DIR = Path(__file__).resolve().parent.parent / "tasks"
# Directories from --tasks-dir, searched after TASKS_DIR and before
# JOUET_BENCH_TASKS.
EXTRA_DIRS: list[Path] = []
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
    source: dict[str, str | bool] | None = None
    opt_in: bool = False
    remote: bool = True
    upstream: bool = False
    upstream_url: str | None = None
    check_timeout_s: int = 120
    tools: dict[str, str] = field(default_factory=dict)
    env: dict[str, str] = field(default_factory=dict)
    path: list[str] = field(default_factory=list)
    setup: list[str] = field(default_factory=list)
    isolated_setup: list[list[str]] = field(default_factory=list)
    # Commands timed once for the cost note runs start with (see costs.py).
    cost_seed: list[str] = field(default_factory=list)
    # A fully pinned pip requirements file, relative to the task directory.
    python_requirements: str | None = None

    def agent_env(self, env: dict[str, str], bin_dir: Path, workdir: Path) -> None:
        """Adds the task's tools, variables and PATH entries to a run's environment."""
        from .arms import resolve, wrapper

        for name, target in self.tools.items():
            link = bin_dir / name
            if "/" not in target:
                if link.exists():
                    continue
                found = resolve(target)
                if found is None:
                    raise RuntimeError(f"task {self.name} needs {target}, which is not on PATH")
                wrapper(link, found, {})
            else:
                # A path replaces a default tool: an imported task's own
                # Python stands in for the host's `python3`.
                path = Path(expand(target, workdir))
                if not path.is_absolute():
                    path = workdir / path
                if not path.exists():
                    raise RuntimeError(f"task {self.name} needs {path}, which does not exist")
                link.unlink(missing_ok=True)
                wrapper(link, path, {})
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


def directories() -> list[Path]:
    """The directories tasks are loaded from, each once."""
    listed = os.environ.get("JOUET_BENCH_TASKS", "")
    found: list[Path] = []
    for d in [TASKS_DIR, *EXTRA_DIRS, *(Path(p) for p in listed.split(os.pathsep) if p)]:
        d = d.expanduser().resolve()
        if not d.is_dir():
            raise ValueError(f"no task directory at {d}")
        if d not in found:
            found.append(d)
    return found


def index() -> dict[str, Path]:
    """Each task's name and directory; a name in two directories is an error."""
    tasks: dict[str, Path] = {}
    for d in directories():
        for p in sorted(d.iterdir()):
            if not (p / "task.toml").exists():
                continue
            if p.name in tasks:
                raise ValueError(f"task {p.name} is in both {tasks[p.name].parent} and {d}")
            tasks[p.name] = p
    return tasks


def load(name: str) -> Task:
    d = index().get(name)
    if d is None:
        raise ValueError(f"no task named {name}")
    with (d / "task.toml").open("rb") as f:
        raw = tomllib.load(f)
    for required in ("check.py", "solve.py") if "source" in raw else ("check.py", "solve.py", "repo"):
        if not (d / required).exists():
            raise ValueError(f"task {name} has no {required}")
    isolated_setup = raw.get("isolated_setup", [])
    if not isinstance(isolated_setup, list) or any(
        not isinstance(command, list) or not command or any(not isinstance(arg, str) or not arg for arg in command)
        for command in isolated_setup
    ):
        raise ValueError(f"task {name}: isolated_setup must be a list of nonempty argument lists")
    source = raw.get("source")
    if source is not None:
        if not isinstance(source, dict) or any(not isinstance(source.get(key), str) or not source[key] for key in ("git", "commit")):
            raise ValueError(f"task {name}: source needs nonempty git and commit strings")
        if not isinstance(source.get("history", False), bool):
            raise ValueError(f"task {name}: source.history must be a boolean")
        if "submodule_base_url" in source and (not isinstance(source["submodule_base_url"], str)
                                                or not source["submodule_base_url"]):
            raise ValueError(f"task {name}: source.submodule_base_url must be a nonempty string")
        if source.get("history") and "submodule_base_url" in source:
            raise ValueError(f"task {name}: history mode does not fetch submodules")
        if source.get("history") and raw.get("remote", True):
            raise ValueError(f"task {name}: source.history requires remote = false")
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
        source=source,
        opt_in=bool(raw.get("opt_in", False)),
        remote=bool(raw.get("remote", True)),
        upstream=bool(raw.get("upstream", False)),
        upstream_url=raw.get("upstream_url"),
        check_timeout_s=int(raw.get("check_timeout_s", 120)),
        tools=dict(raw.get("tools", {})),
        env=dict(raw.get("env", {})),
        path=list(raw.get("path", [])),
        setup=list(raw.get("setup", [])),
        isolated_setup=isolated_setup,
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


def requirements_digest(task: Task, requirements: Path) -> str:
    try:
        return hashlib.sha256(requirements.read_bytes()).hexdigest()
    except OSError as error:
        raise EnvironmentSetupError(f"{task.name}: cannot read Python requirements {requirements}: {error}") from error


def setup_checkout_state(workdir: Path) -> tuple[bytes, bytes, list[tuple[bytes, int, str]]]:
    """Include untracked contents as well as the tracked diff and path list."""
    excluded = {"GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS", "GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE",
                "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"}
    env = {k: v for k, v in os.environ.items() if k not in excluded
           and not k.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))} | GIT_ENV

    def git_output(*args: str) -> bytes:
        return subprocess.run(
            ["git", "-c", f"core.excludesFile={os.devnull}", *args], cwd=workdir,
            env=env, capture_output=True, check=True, timeout=30,
        ).stdout

    untracked = git_output("ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
    files = []
    for name in untracked:
        if not name:
            continue
        path = workdir / os.fsdecode(name)
        digest = hashlib.sha256()
        if path.is_symlink():
            digest.update(os.fsencode(os.readlink(path)))
        else:
            with path.open("rb") as contents:
                for chunk in iter(lambda: contents.read(1024 * 1024), b""):
                    digest.update(chunk)
        files.append((name, path.lstat().st_mode, digest.hexdigest()))
    return (git_output("status", "--porcelain", "--ignored=no", "-z"),
            git_output("diff", "--no-ext-diff", "--no-textconv", "--binary", "HEAD"), files)


def isolated_setup(task: Task, env: dict[str, str], workdir: Path, log: Path) -> None:
    """Run a task's setup in its own environment, before the agent starts."""
    if not task.isolated_setup:
        return
    try:
        before = setup_checkout_state(workdir)
        with log.open("a") as output:
            for args in task.isolated_setup:
                command = [expand(arg, workdir).replace("{taskdir}", str(task.dir.resolve())) for arg in args]
                proc = subprocess.run(command, cwd=workdir, env=env, stdout=output, stderr=subprocess.STDOUT,
                                      timeout=600)
                if proc.returncode:
                    raise EnvironmentSetupError(f"{task.name}: isolated setup failed (log: {log})")
        after = setup_checkout_state(workdir)
    except (OSError, subprocess.SubprocessError) as error:
        raise EnvironmentSetupError(f"{task.name}: isolated setup failed: {error} (log: {log})") from error
    if after != before:
        raise EnvironmentSetupError(f"{task.name}: isolated setup changed files in the checkout (log: {log})")


def setup_environment(task: Task, env: dict[str, str], workdir: Path, cache: Path, log: Path) -> dict[str, str] | None:
    """Prepare the run's isolated environment before an agent or check starts."""
    requirements = requirements_file(task)
    if requirements is None:
        isolated_setup(task, env, workdir, log)
        return {"setup_commands": str(len(task.isolated_setup))} if task.isolated_setup else None
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
    isolated_setup(task, env, workdir, log)
    result = {"python": version.stdout.strip(), "requirements_sha256": requirements_digest(task, requirements)}
    if task.isolated_setup:
        result["setup_commands"] = str(len(task.isolated_setup))
    return result


def expand(value: str, workdir: Path) -> str:
    if value.startswith("~/"):
        value = str(Path.home() / value[2:])
    return value.replace("{workdir}", str(workdir))


def all_names() -> list[str]:
    return sorted(index())


def git_env() -> dict[str, str]:
    redirected = {"GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS", "GIT_DIR", "GIT_WORK_TREE",
                  "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR"}
    return {k: v for k, v in os.environ.items() if k not in redirected
            and not k.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))} | GIT_ENV


def git(workdir: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=workdir, env=git_env(), check=True, capture_output=True, text=True
    ).stdout.strip()


def checkout_roots(workdir: Path) -> list[Path]:
    """All worktrees sharing this checkout, including paths with whitespace."""
    output = subprocess.run(
        ["git", "worktree", "list", "--porcelain", "-z"],
        cwd=workdir, env=git_env(), check=True, capture_output=True, text=True,
    ).stdout
    roots = [Path(item.removeprefix("worktree ")).resolve() for item in output.split("\0")
             if item.startswith("worktree ")]
    if not roots:
        raise RuntimeError("git returned no worktrees")
    return roots


def clean_submodule_sources(workdir: Path) -> None:
    """Remove resolved source locations from fresh submodule Git metadata."""
    output = git(workdir, "submodule", "foreach", "--quiet", "--recursive",
                 'printf "%s\\0" "$toplevel/$sm_path"')
    submodules = [Path(path) for path in output.split("\0") if path]
    for repo in [workdir, *submodules]:
        result = subprocess.run(
            ["git", "config", "--local", "--null", "--name-only", "--get-regexp", r"^submodule\..*\.url$"],
            cwd=repo, env=git_env(), capture_output=True, text=True,
        )
        if result.returncode not in (0, 1):
            raise subprocess.CalledProcessError(result.returncode, result.args, result.stdout, result.stderr)
        for key in result.stdout.split("\0"):
            if key:
                git(repo, "config", "--local", "--unset-all", key)
        if repo != workdir:
            git(repo, "remote", "remove", "origin")
            git(repo, "reflog", "expire", "--expire=all", "--all")
            fetch_head = Path(git(repo, "rev-parse", "--git-path", "FETCH_HEAD"))
            (repo / fetch_head).unlink(missing_ok=True)


def prepare(task: Task, workdir: Path) -> dict[str, str]:
    """Builds the task's work tree in `workdir`; returns its SHAs."""
    if task.source and task.source.get("history", False) and task.remote:
        raise EnvironmentSetupError("history mode requires remote = false")
    if task.source:
        if task.source.get("history", False):
            copy_history(task.source, workdir)
        else:
            copy_source(task.source, workdir)
    else:
        shutil.copytree(task.dir / "repo", workdir, ignore=IGNORE)
        git(workdir, "init", "-q", "-b", "main")
        git(workdir, "add", "-A")
    if not task.source or not task.source.get("history", False):
        git(workdir, "commit", "-q", "-m", "initial commit")
    # An archived tree contains gitlinks, not the files they point to. Populate
    # them at the recorded revisions so an agent can run the project's tests.
    if task.source and (workdir / ".gitmodules").exists():
        if task.source.get("history", False):
            raise EnvironmentSetupError("history mode does not fetch submodules; provide a task without them")
        base = task.source.get("submodule_base_url")
        if base:
            # Only setup uses this URL. The task's origin is installed below,
            # after relative .gitmodules URLs have been resolved.
            git(workdir, "remote", "add", "origin", str(base))
        try:
            git(workdir, "submodule", "update", "--init", "--recursive")
            if base:
                clean_submodule_sources(workdir)
        finally:
            if base:
                git(workdir, "remote", "remove", "origin")
    run_setup(task, workdir)
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
        git(workdir, "remote", "add", "origin", task.upstream_url or str(origin))
        if task.upstream_url:
            # The URL a clone would have, resolved to the local repository.
            git(workdir, "config", f"url.{origin}.insteadOf", task.upstream_url)
        git(workdir, "push", "-q", "origin", f"{base}:refs/heads/main")
        git(workdir, "fetch", "-q", "origin")
        git(workdir, "branch", "-q", "--set-upstream-to=origin/main", "main")
        git(workdir, "remote", "set-head", "origin", "main")
        if task.source and task.source.get("history", False):
            git(workdir, "remote", "set-url", "--push", "origin", os.devnull)
    for rel, text in task.untracked.items():
        path = workdir / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return {"base_sha": base, "head_sha": head}


def run_setup(task: Task, workdir: Path) -> None:
    """Runs the task's `setup` commands, such as installing a project's
    dependencies offline, and refuses changes to the initial work tree."""
    if not task.setup:
        return
    before = (git(workdir, "status", "--porcelain", "--ignored=no"), git(workdir, "diff", "--binary", "HEAD"))
    for command in task.setup:
        proc = subprocess.run(
            [expand(arg, workdir) for arg in shlex.split(command)],
            cwd=workdir,
            capture_output=True,
            text=True,
            timeout=600,
        )
        if proc.returncode != 0:
            output = (proc.stdout + proc.stderr)[-3000:]
            raise RuntimeError(f"task {task.name}: setup `{command}` exited {proc.returncode}:\n{output}")
    dirty = git(workdir, "status", "--porcelain", "--ignored=no")
    if (dirty, git(workdir, "diff", "--binary", "HEAD")) != before:
        raise RuntimeError(f"task {task.name}: setup changed files git does not ignore:\n{dirty}")


def copy_source(source: dict[str, str | bool], workdir: Path) -> None:
    """Stages the exact commit tree in a new, independent repository.

    Import the objects and check out the index directly: `git archive` can
    omit export-ignored files, and re-adding an archive can normalize blobs
    whose bytes do not match the project's current attributes.
    """
    repo = Path(str(source["git"])).expanduser().resolve()
    commit = str(source["commit"])
    revision = git(repo, "rev-parse", f"{commit}^{{commit}}")
    workdir.mkdir(parents=True)
    git(workdir, "init", "-q", "-b", "main")
    git(workdir, "fetch", "-q", "--no-tags", "--no-write-fetch-head", "--depth=1", str(repo), revision)
    git(workdir, "read-tree", "--reset", "-u", f"{revision}^{{tree}}")
    tree = git(workdir, "write-tree")
    want = git(repo, "rev-parse", f"{commit}^{{tree}}")
    if tree != want:
        raise RuntimeError(f"the staged tree {tree} is not {commit}'s tree {want}")


def helpers_env() -> dict[str, str]:
    """Where `_check.py` is, for a task outside `bench/tasks/` to import."""
    return {"BENCH_CHECK_HELPERS": str(TASKS_DIR)}


def copy_history(source: dict[str, str | bool], workdir: Path) -> None:
    """Copy a pinned commit and its ancestors without retaining a network remote."""
    repo = Path(str(source["git"])).expanduser().resolve()
    commit = str(source["commit"])
    if not re.fullmatch(r"[0-9a-f]{40}(?:[0-9a-f]{24})?", commit):
        raise EnvironmentSetupError("history mode needs a full pinned commit SHA")
    try:
        resolved = git(repo, "rev-parse", f"{commit}^{{commit}}")
        shallow = git(repo, "rev-parse", "--is-shallow-repository")
    except (OSError, subprocess.CalledProcessError) as error:
        raise EnvironmentSetupError("history mode cannot read the pinned source commit") from error
    if resolved != commit:
        raise EnvironmentSetupError("history mode source.commit must identify a commit")
    if shallow == "true":
        raise EnvironmentSetupError("history mode needs a complete, non-shallow source repository")
    partial = subprocess.run(
        ["git", "config", "--local", "--get-regexp", r"^remote\..*\.promisor$"],
        cwd=repo, env=git_env(), capture_output=True, text=True,
    )
    if partial.returncode not in (0, 1):
        raise EnvironmentSetupError("could not inspect the source repository's partial-clone configuration")
    if partial.returncode == 0 and any(line.split()[-1].lower() == "true" for line in partial.stdout.splitlines()):
        raise EnvironmentSetupError("history mode needs a complete source, not a partial clone")
    workdir.mkdir(parents=True)
    git(workdir, "init", "-q", "-b", "main")
    try:
        git(workdir, "fetch", "-q", "--no-tags", "--no-write-fetch-head", str(repo), commit)
        git(workdir, "checkout", "-q", "-B", "main", commit)
    except (OSError, subprocess.CalledProcessError) as error:
        raise EnvironmentSetupError("history mode could not copy the pinned source history") from error
    if git(workdir, "rev-parse", "HEAD") != commit:
        raise EnvironmentSetupError("history mode did not check out the pinned commit")


def check_env(shas: dict[str, str], state: Path | None) -> dict[str, str]:
    env = {"BENCH_BASE_SHA": shas["base_sha"], "BENCH_HEAD_SHA": shas["head_sha"], **helpers_env()}
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
        return {"passed": None, "exit_code": None, "output": "check timed out", "reason": "check timed out"}
    output = (proc.stdout + proc.stderr).strip()
    marker = "JOUET_BENCH_UNVERIFIED_V1"
    stdout = proc.stdout.rstrip()
    if proc.returncode == 3 and stdout.splitlines()[-1:] == [marker]:
        output = (stdout[: -len(marker)].rstrip() + "\n" + proc.stderr).strip()
        return {"passed": None, "exit_code": 3, "output": output, "reason": "required checks were not verified"}
    return {"passed": proc.returncode == 0, "exit_code": proc.returncode, "output": output}
