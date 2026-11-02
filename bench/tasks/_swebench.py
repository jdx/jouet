"""The check and reference solution of a task imported from a SWE-bench-format
record by `bench/import_swebench.py`.

The task directory holds `swebench.json` (the instance's id, its tests and how
to run them), `hidden/` (the test files as the instance's `test_patch` leaves
them) and `fix.patch` (the instance's gold `patch`). This runs under the
task's own Python, which can be as old as 3.8, so it keeps to what 3.8 has.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _check import commit, expect, finish, new_commit  # noqa: E402

PASS, FAIL, SKIP = "pass", "fail", "skip"


def load(task_dir: Path) -> dict:
    return json.loads((task_dir / "swebench.json").read_text())


def parse_django(log: str) -> dict[str, str]:
    """Django's runner at `--verbosity 2`: `name ... ok`, where name is
    `test (module.Class)` or a docstring's first line, as SWE-bench names
    them. A test that prints puts its status on a line of its own."""
    statuses = {}
    pending = None
    for line in log.splitlines():
        m = re.match(r"^(.+?) \.\.\. (.*)$", line)
        word = (m.group(2) if m else line).strip()
        name = m.group(1) if m else pending
        status = django_status(word)
        if m and status is None:
            pending = name
            continue
        if name is not None and status is not None:
            statuses[name] = status
            pending = None
    return statuses


def django_status(word: str) -> str | None:
    if word in ("ok", "OK", "expected failure"):
        return PASS
    if word in ("FAIL", "ERROR", "unexpected success"):
        return FAIL
    if word.startswith("skipped"):
        return SKIP
    return None


def parse_sympy(log: str) -> dict[str, str]:
    """sympy's `bin/test --verbose`: `test_name ok`, `F`, `E`, `f` (an
    expected failure), `X` (an unexpected pass) or `s`."""
    statuses = {}
    for m in re.finditer(r"^(test_\w+) (ok|F|E|f|X|s|Skipped)\b", log, re.M):
        statuses[m.group(1)] = {"ok": PASS, "f": PASS, "s": SKIP, "Skipped": SKIP}.get(m.group(2), FAIL)
    return statuses


def parse_pytest(log: str) -> dict[str, str]:
    """pytest's `-rA` summary: `PASSED nodeid`, `FAILED nodeid - reason`.
    A node id is cut at its first space, as SWE-bench names tests."""
    statuses = {}
    for m in re.finditer(r"^(PASSED|FAILED|ERROR|XFAIL|XPASS|SKIPPED) (\S+)", log, re.M):
        if m.group(1) == "SKIPPED":
            continue  # `SKIPPED [1] path:line: reason` names no test.
        statuses[m.group(2)] = PASS if m.group(1) in ("PASSED", "XFAIL") else FAIL
    return statuses


PARSERS = {"django": parse_django, "sympy": parse_sympy, "pytest": parse_pytest}


def verdict(spec: dict, statuses: dict[str, str]) -> list[str]:
    """What failed: every FAIL_TO_PASS test must pass; a PASS_TO_PASS test
    must not fail. One the environment skips or the log does not name is
    reported, but does not fail the check."""
    problems = []
    for name in spec["fail_to_pass"]:
        status = statuses.get(name)
        if status != PASS:
            problems.append(f"FAIL_TO_PASS {name}: {status or 'not run'}")
    absent = []
    for name in spec["pass_to_pass"]:
        status = statuses.get(name)
        if status == FAIL:
            problems.append(f"PASS_TO_PASS {name}: fail")
        elif status is None:
            absent.append(name)
    if absent:
        print(f"{len(absent)} PASS_TO_PASS tests were not run or not named in the log: {absent[:5]}"
              + (f" ... {len(absent) - 5} more" if len(absent) > 5 else ""))
    return problems


def place_hidden(task_dir: Path, spec: dict) -> None:
    """The instance's tests replace whatever is in the work tree."""
    hidden = task_dir / "hidden"
    for rel in spec["test_files"]:
        target = Path(rel)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(hidden / rel, target)
    for rel in spec.get("deleted_test_files", []):
        if os.path.lexists(rel):
            os.remove(rel)


def check(task_dir: Path) -> None:
    spec = load(task_dir)
    place_hidden(task_dir, spec)
    env = dict(os.environ)
    if spec.get("pythonpath") is not None:
        env["PYTHONPATH"] = str(Path.cwd() / spec["pythonpath"])
    command = [sys.executable if arg == "{python}" else arg for arg in spec["test_command"]]
    try:
        proc = subprocess.run(command, capture_output=True, text=True, env=env, timeout=spec.get("test_timeout_s", 600))
    except subprocess.TimeoutExpired:
        expect(False, f"the tests timed out: {' '.join(command)}")
        finish()
        return
    log = proc.stdout + proc.stderr
    Path(os.environ.get("TMPDIR", "/tmp"), f"{spec['instance_id']}.test.log").write_text(log)
    parser = PARSERS.get(spec["log_format"])
    if parser is None:
        expect(proc.returncode == 0, f"the tests exited {proc.returncode}:\n{log[-3000:]}")
    else:
        statuses = parser(log)
        problems = verdict(spec, statuses)
        print(f"{sum(s == PASS for s in statuses.values())} of {len(statuses)} tests in the log passed")
        expect(not problems, "the instance's tests fail:\n" + "\n".join(problems) + f"\n{log[-2500:]}")
    # Not a condition of the check: the prompt asks for a commit.
    print(f"committed: {new_commit()}")
    finish()


def solve(task_dir: Path) -> None:
    spec = load(task_dir)
    subprocess.run(["git", "apply", str(task_dir / "fix.patch")], check=True)
    commit(f"Fix {spec['instance_id']}", *spec["patch_files"])
