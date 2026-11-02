#!/usr/bin/env python3
"""Turn SWE-bench-format records into opt-in benchmark tasks.

    python3 bench/import_swebench.py import RECORDS --ids ID[,ID...] [--dataset NAME]
    python3 bench/import_swebench.py prime bench/tasks/swebench-django-11400 [...]

`RECORDS` is a JSONL file (a Hugging Face dataset's `to_json` export), a
JSON list, or a page of the dataset viewer's `/rows` API. Records follow
SWE-bench's fields (`instance_id`, `repo`, `base_commit`,
`problem_statement`, `patch`, `test_patch`, `FAIL_TO_PASS`,
`PASS_TO_PASS`), as SWE-bench Verified, SWE-bench Multilingual and
SWE-PolyBench do; Multi-SWE-bench's own field names are mapped onto them.

`import` clones each repository once under `--clones` (if missing) and
writes one task per instance: the prompt is the problem statement, `hidden/`
holds the test files as `test_patch` leaves them, `check.py` runs the
instance's tests and requires every FAIL_TO_PASS test to pass and no
PASS_TO_PASS test to fail, and `solve.py` applies the gold patch.

`prime` makes each task runnable offline: it builds a Python environment for
the instance under `--envs`, with the repository's own dependencies as of
six months after the instance's date and its test extras, and makes it
read-only. Runs then put the
work tree first on `PYTHONPATH`, so the agent's edits are what is tested. Only
`prime` uses the network.

See docs/benchmarks.md.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path

TASKS_DIR = Path(__file__).resolve().parent / "tasks"
CLONES = Path("~/src/swebench")
ENVS = Path("~/src/swebench/envs")
# An environment gets the packages released by this long after the instance.
CUTOFF_MARGIN = timedelta(days=180)
BUILD_TOOLS_CUTOFF = "2024-01-01T00:00:00Z"
# The file in an environment that says `prime` finished it.
PRIMED = ".jouet-primed"
# The line every task from a real project ends its prompt with.
ASK = "Fix this in {project}, with a test, and commit."


@dataclass
class Spec:
    """How to run a repository's tests at a version: the Python to build its
    environment with (None for a project that is not Python), extra packages
    its tests need, where its package sits for PYTHONPATH, the test runner,
    and how to read the runner's log."""

    python: str | None
    runner: str
    log_format: str
    extras: list[str] = field(default_factory=list)
    pythonpath: str | None = ""
    # The runner `command`'s own argument list.
    command: list[str] = field(default_factory=list)
    # The task's `setup` commands, which may write only what git ignores.
    setup: list[str] = field(default_factory=list)


# Pure-Python repositories whose tests import the work tree directly. Python
# versions follow SWE-bench's harness, except Django 3.x, which SWE-bench
# runs on 3.6: uv's Python builds start at 3.8, which Django 3.0 supports.
# Extras are pure Python, so they install for any Python of the instance's
# date; tests that need a compiled extra skip themselves.
def spec_for(repo: str, version: str) -> Spec | None:
    if repo == "django/django":
        python = {"4.1": "3.9", "4.2": "3.9", "5.0": "3.11", "5.1": "3.11", "5.2": "3.12"}.get(version, "3.8")
        return Spec(python, "django", "django", ["tblib", "Jinja2", "docutils", "PyYAML"])
    if repo == "sympy/sympy":
        return Spec("3.9", "sympy", "sympy", ["mpmath"])
    if repo == "sphinx-doc/sphinx":
        return Spec("3.9", "pytest", "pytest", ["pytest", "html5lib"])
    if repo == "pylint-dev/pylint":
        return Spec("3.9", "pytest", "pytest", ["pytest"])
    if repo == "pytest-dev/pytest":
        # The `_version.py` an install generates, which git ignores.
        write = f"open('src/_pytest/_version.py', 'w').write('version = %r\\n' % {version!r})"
        return Spec("3.9", "pytest", "pytest", ["hypothesis", "xmlschema"], "src",
                    setup=[f'python3 -c "{write}"'])
    return None


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def git(cwd: Path, *args: str, env: dict[str, str] | None = None, input: bytes | None = None) -> bytes:
    return subprocess.run(["git", *args], cwd=cwd, env={**os.environ, **(env or {})}, input=input,
                          capture_output=True, check=True).stdout


# --- records ---------------------------------------------------------------

def read_records(path: Path) -> list[dict]:
    text = path.read_text()
    if path.suffix == ".jsonl":
        raw = [json.loads(line) for line in text.splitlines() if line.strip()]
    else:
        data = json.loads(text)
        raw = [r.get("row", r) for r in data["rows"]] if isinstance(data, dict) and "rows" in data else data
    return [normalize(r) for r in raw]


def as_list(value) -> list[str]:
    """A test list, which Hugging Face exports carry as a JSON string."""
    if isinstance(value, str):
        value = json.loads(value) if value.strip() else []
    if isinstance(value, dict):
        value = sorted(value)
    return list(value or [])


def normalize(r: dict) -> dict:
    """A record in SWE-bench's field names, from SWE-bench's own or
    Multi-SWE-bench's (`org`, `repo`, `number`, `base.sha`, `fix_patch`,
    `resolved_issues`, `f2p_tests`, `p2p_tests`)."""
    if "fix_patch" in r and "patch" not in r:
        repo = f"{r['org']}/{r['repo']}"
        issues = r.get("resolved_issues") or []
        statement = "\n\n".join(f"{i.get('title', '')}\n\n{i.get('body') or ''}".strip() for i in issues)
        r = {
            "instance_id": r.get("instance_id") or f"{r['org']}__{r['repo']}-{r['number']}",
            "repo": repo,
            "base_commit": r["base"]["sha"],
            "problem_statement": statement or r.get("body") or r.get("title", ""),
            "patch": r["fix_patch"],
            "test_patch": r["test_patch"],
            "FAIL_TO_PASS": r.get("f2p_tests", {}),
            "PASS_TO_PASS": r.get("p2p_tests", {}),
            "version": r.get("version", ""),
            "created_at": r.get("created_at", ""),
        }
    out = dict(r)
    out["FAIL_TO_PASS"] = as_list(r.get("FAIL_TO_PASS"))
    out["PASS_TO_PASS"] = as_list(r.get("PASS_TO_PASS"))
    out["version"] = str(r.get("version") or "")
    for key in ("instance_id", "repo", "base_commit", "problem_statement", "patch", "test_patch"):
        if not out.get(key):
            raise ValueError(f"record {r.get('instance_id', '?')} has no {key}")
    # They become paths and git arguments, and records are downloaded data.
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", out["instance_id"]) or ".." in out["instance_id"]:
        raise ValueError(f"record {out['instance_id']!r}: not an instance id")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*", out["repo"]) or ".." in out["repo"]:
        raise ValueError(f"record {out['instance_id']}: {out['repo']!r} is not owner/name")
    if not re.fullmatch(r"[0-9a-f]{7,40}", out["base_commit"]):
        raise ValueError(f"record {out['instance_id']}: {out['base_commit']!r} is not a commit")
    return out


# --- the task's files --------------------------------------------------------

def task_name(instance_id: str) -> str:
    """`django__django-11400` becomes `swebench-django-11400`."""
    return "swebench-" + re.sub(r"[^A-Za-z0-9._-]+", "-", instance_id.split("__", 1)[-1]).lower()


def clone(repo: str, clones: Path, commit: str) -> Path:
    """A local clone holding `commit`, made once."""
    path = clones.expanduser() / repo.replace("/", "__")
    if not path.exists():
        log(f"cloning {repo} into {path}")
        path.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "-q", f"https://github.com/{repo}.git", str(path)], check=True)
    if subprocess.run(["git", "cat-file", "-e", f"{commit}^{{commit}}"], cwd=path, capture_output=True).returncode:
        git(path, "fetch", "-q", "origin", commit)
    return path


def patch_files(repo: Path, base: str, patch: str) -> tuple[dict[str, bytes], list[str]]:
    """The files `patch` leaves when applied to `base`, and the ones it
    deletes, applied in a scratch index so the clone's own is untouched."""
    with tempfile.TemporaryDirectory() as tmp:
        env = {"GIT_INDEX_FILE": str(Path(tmp) / "index")}
        git(repo, "read-tree", base, env=env)
        git(repo, "apply", "--cached", "-", env=env, input=patch.encode())
        status = git(repo, "diff", "--cached", "--name-status", "--no-renames", "-z", base, env=env).decode()
        fields = status.split("\0")
        files, deleted = {}, []
        for kind, path in zip(fields[0::2], fields[1::2]):
            if kind == "D":
                deleted.append(path)
            else:
                files[path] = git(repo, "show", f":{path}", env=env)
    return files, deleted


def test_command(spec: Spec, record: dict, test_files: list[str]) -> list[str]:
    """The runner over the instance's test modules, as SWE-bench runs it."""
    py = [f for f in test_files if f.endswith(".py")]
    if spec.runner == "django":
        labels = []
        for f in py:
            parts = Path(f).parts
            name = parts[-1]
            if parts[0] != "tests" or not (name.startswith("test") and name != "test_sqlite.py"):
                continue
            labels.append(".".join(parts[1:])[:-3])
        return ["{python}", "tests/runtests.py", "--verbosity", "2", "--settings=test_sqlite", "--parallel", "1",
                *labels]
    if spec.runner == "sympy":
        return ["{python}", "bin/test", "-C", "--verbose",
                *[f for f in py if re.search(r"(^|/)tests/test_[^/]+\.py$", f)]]
    if spec.runner == "pytest":
        # The files the instance's tests are in, as their node ids name them.
        files = sorted({t.split("::", 1)[0] for t in record["FAIL_TO_PASS"] + record["PASS_TO_PASS"] if "::" in t})
        return ["{python}", "-m", "pytest", "-rA", "--tb=short", "-p", "no:cacheprovider", *files]
    if spec.runner == "command":
        return spec.command
    raise ValueError(f"unknown runner {spec.runner}")


def toml_string(text: str) -> str:
    """A multi-line basic TOML string that reads back as `text`."""
    text = text.replace("\r\n", "\n").replace("\\", "\\\\").replace('"""', '""\\"')
    text = re.sub(r"[\x00-\x08\x0b-\x1f\x7f]", lambda m: f"\\u{ord(m.group()):04x}", text)
    return f'"""\n{text}"""'


def write_task(record: dict, out: Path, clones: Path, envs: Path, dataset: str, spec: Spec) -> Path:
    instance = record["instance_id"]
    repo = clone(record["repo"], clones, record["base_commit"])
    tests, deleted = patch_files(repo, record["base_commit"], record["test_patch"])
    fixed, removed = patch_files(repo, record["base_commit"], record["patch"])
    files = sorted(tests)
    command = test_command(spec, record, files)
    # With no test targets, a runner would run the project's whole suite.
    if spec.runner != "command" and len(command) == len(test_command(spec, {"FAIL_TO_PASS": [], "PASS_TO_PASS": []}, [])):
        raise SystemExit(f"{instance}: no test modules to run from its test_patch or test names")
    task = out / task_name(instance)
    if task.exists():
        shutil.rmtree(task)
    (task / "hidden").mkdir(parents=True)
    for rel, data in tests.items():
        (task / "hidden" / rel).parent.mkdir(parents=True, exist_ok=True)
        (task / "hidden" / rel).write_bytes(data)
    (task / "fix.patch").write_text(record["patch"])
    meta = {
        "instance_id": instance,
        "dataset": dataset,
        "repo": record["repo"],
        "base_commit": record["base_commit"],
        "version": record["version"],
        "created_at": record.get("created_at", ""),
        "difficulty": record.get("difficulty", ""),
        "python": spec.python,
        "extras": spec.extras,
        "pythonpath": spec.pythonpath,
        "test_command": command,
        "log_format": spec.log_format,
        "test_files": files,
        "deleted_test_files": deleted,
        # What the gold patch changes, deletions included, for `solve.py` to commit.
        "patch_files": sorted([*fixed, *removed]),
        "fail_to_pass": record["FAIL_TO_PASS"],
        "pass_to_pass": record["PASS_TO_PASS"],
    }
    (task / "swebench.json").write_text(json.dumps(meta, indent=2) + "\n")
    for script, fn in (("check.py", "check"), ("solve.py", "solve")):
        (task / script).write_text(
            "import sys\nfrom pathlib import Path\n\n"
            "sys.path.insert(0, str(Path(__file__).resolve().parent.parent))\n"
            f"from _swebench import {fn}  # noqa: E402\n\n"
            f"{fn}(Path(__file__).resolve().parent)\n"
        )
    project = record["repo"].split("/")[-1]
    title = record["problem_statement"].strip().splitlines()[0].strip()[:100] if record["problem_statement"].strip() else instance
    # Absolute, since a run resolves them from its own work tree.
    clones, envs = clones.expanduser().absolute(), envs.expanduser().absolute()
    home = "~/" + str(clones.relative_to(Path.home())) if clones.is_relative_to(Path.home()) else str(clones)
    env_dir = envs / instance
    env_path = "~/" + str(env_dir.relative_to(Path.home())) if env_dir.is_relative_to(Path.home()) else str(env_dir)
    lines = [
        # Not ASCII-escaped: TOML refuses the surrogate pairs json writes for emoji.
        f"description = {json.dumps(f'{dataset} {instance}: {title}', ensure_ascii=False)}",
        'chore = "bug-fix"',
        'rules = ["checks", "git-add-all"]',
        f"# Generated by bench/import_swebench.py from {dataset} {instance}",
        f"# ({record.get('difficulty') or 'no difficulty label'}); the gold patch touches "
        f"{len(fixed)} files: {', '.join(sorted(fixed))}.",
        "# `check.py` runs the instance's tests (swebench.json) with hidden/ in place.",
        "opt_in = true",
        "remote = false",
        "upstream = true",
        "timeout_s = 1800",
        "check_timeout_s = 900",
        *([f"setup = {json.dumps(spec.setup)}"] if spec.setup else []),
        "# The instance's problem statement as its author wrote it, and one line.",
        f"prompt = {toml_string(record['problem_statement'].strip() + chr(10) + chr(10) + ASK.format(project=project) + chr(10))}",
        "",
        "[source]",
        f'git = "{home}/{record["repo"].replace("/", "__")}"',
        f'commit = "{record["base_commit"]}"',
    ]
    if spec.python:
        lines += [
            "",
            "# The environment `python3 bench/import_swebench.py prime` builds, read-only.",
            "[tools]",
            f'python3 = "{env_path}/bin/python"',
            f'python = "{env_path}/bin/python"',
            "",
            "[env]",
            f'PYTHONPATH = "{{workdir}}{"/" + spec.pythonpath if spec.pythonpath else ""}"',
        ]
    (task / "task.toml").write_text("\n".join(lines) + "\n")
    return task


# --- the environment --------------------------------------------------------

def prime(task: Path, clones: Path, envs: Path) -> Path | None:
    """The instance's Python environment: the repository at its base commit
    installed with its dependencies as of the instance's date, and the test
    extras. Read-only, so a run cannot change what the next one gets."""
    meta = json.loads((task / "swebench.json").read_text())
    repo = clone(meta["repo"], clones, meta["base_commit"])
    if not meta.get("python"):
        log(f"{task.name}: not a Python project; only the clone was primed")
        return None
    env = envs.expanduser() / meta["instance_id"]
    # Written last, so an interrupted prime is built again.
    primed = env / PRIMED
    if primed.exists():
        log(f"{task.name}: {env} is already primed")
        return env
    uv = shutil.which("uv")
    if not uv:
        raise SystemExit("prime needs uv (MISE_ENV=bench mise x -- python3 bench/import_swebench.py prime ...)")
    if env.exists():
        subprocess.run(["chmod", "-R", "u+w", str(env)], check=True)
        shutil.rmtree(env)
    # Packages as of a while after the instance: a base commit between
    # releases can need a dependency released after the issue was filed.
    newer = []
    if meta.get("created_at"):
        cutoff = datetime.fromisoformat(meta["created_at"].replace("Z", "+00:00")) + CUTOFF_MARGIN
        newer = ["--exclude-newer", cutoff.strftime("%Y-%m-%dT%H:%M:%SZ")]
        # Except the build tools, which only build the installed copy: one
        # new enough for any build backend, old enough to have pkg_resources.
        for tool in ("setuptools", "wheel"):
            newer += ["--exclude-newer-package", f"{tool}={BUILD_TOOLS_CUTOFF}"]
        # atomicwrites' author deleted its old releases from PyPI in 2022 and
        # uploaded 1.4.1 again; no earlier cutoff finds it.
        newer += ["--exclude-newer-package", "atomicwrites=2022-12-31T00:00:00Z"]
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        src.mkdir()
        archive = Path(tmp) / "src.tar"
        archive.write_bytes(git(repo, "archive", meta["base_commit"]))
        with tarfile.open(archive) as tar:
            tar.extractall(src, filter="data")
        # The installed copy only brings the dependencies, and a date tag
        # such as Sphinx's `.dev` + `tag_date` is no valid version today.
        setup_cfg = src / "setup.cfg"
        if setup_cfg.exists():
            setup_cfg.write_text(re.sub(r"(?m)^tag_(build|date)\s*=.*\n", "", setup_cfg.read_text()))
        # Built in place: a virtual environment cannot be moved.
        subprocess.run([uv, "venv", "-q", "--python", meta["python"], str(env)], check=True)
        try:
            # An archive has no history for setuptools-scm to read a version from.
            version = {**os.environ, "SETUPTOOLS_SCM_PRETEND_VERSION": meta.get("version") or "0"}
            subprocess.run([uv, "pip", "install", "-q", "--link-mode", "copy", "--python",
                            str(env / "bin" / "python"), *newer, str(src), *meta["extras"]], check=True, env=version)
        except subprocess.CalledProcessError:
            shutil.rmtree(env)
            raise SystemExit(f"{task.name}: could not install its environment (above)") from None
    primed.write_text(f"{meta['instance_id']}\n")
    subprocess.run(["chmod", "-R", "a-w", str(env)], check=True)
    log(f"{task.name}: primed {env}")
    return env


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="bench/import_swebench.py", description=__doc__.split("\n")[0])
    parser.add_argument("--clones", type=Path, default=CLONES, help=f"where repositories are cloned (default {CLONES})")
    parser.add_argument("--envs", type=Path, default=ENVS, help=f"where environments are built (default {ENVS})")
    sub = parser.add_subparsers(dest="command", required=True)
    imp = sub.add_parser("import", help="write a task per instance")
    imp.add_argument("records", type=Path)
    imp.add_argument("--ids", required=True, help="instance ids, comma-separated")
    imp.add_argument("--dataset", default="SWE-bench", help="the dataset's name, for the description")
    imp.add_argument("--out", type=Path, default=TASKS_DIR)
    imp.add_argument("--python", help="the Python to build the environment with, for a repository with no spec")
    imp.add_argument("--extras", default="", help="extra packages for the environment, comma-separated")
    imp.add_argument("--pythonpath", default="", help="the package's directory in the work tree, such as src")
    imp.add_argument("--test-cmd", help="the test command, for a repository with no spec (passes on exit 0); "
                                        "{python} is the environment's Python")
    pri = sub.add_parser("prime", help="build each task's environment, so runs are offline")
    pri.add_argument("tasks", type=Path, nargs="+")
    args = parser.parse_args(argv)

    if args.command == "prime":
        for task in args.tasks:
            prime(task, args.clones, args.envs)
        return 0
    wanted = [i.strip() for i in args.ids.split(",") if i.strip()]
    records = {r["instance_id"]: r for r in read_records(args.records)}
    missing = [i for i in wanted if i not in records]
    if missing:
        raise SystemExit(f"not in {args.records}: {', '.join(missing)}")
    for instance in wanted:
        record = records[instance]
        spec = spec_for(record["repo"], record["version"])
        if args.test_cmd:
            spec = Spec(args.python, "command", "exit", [e for e in args.extras.split(",") if e],
                        args.pythonpath if args.python else None, shlex.split(args.test_cmd))
        if spec is None:
            raise SystemExit(f"{instance}: no spec for {record['repo']}; pass --test-cmd (and --python for Python)")
        print(write_task(record, args.out, args.clones, args.envs, args.dataset, spec))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
