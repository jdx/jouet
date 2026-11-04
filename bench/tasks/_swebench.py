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
from _check import UNVERIFIED, commit, expect, finish, new_commit  # noqa: E402

PASS, FAIL, SKIP, XFAIL = "pass", "fail", "skip", "xfail"


def load(task_dir: Path) -> dict:
    return json.loads((task_dir / "swebench.json").read_text())


def parse_django(log: str) -> dict[str, str]:
    """Django's runner at `--verbosity 2`: `name ... ok`, where name is
    `test (module.Class)` or a docstring's first line, as SWE-bench names
    them. A test that prints puts its status on a line of its own."""
    statuses = {}
    pending = None
    header = None
    for line in log.splitlines():
        failure = re.fullmatch(r"(?:FAIL|ERROR): (test\S+ \([^)]+\))(?: \(.+\))?", line)
        if failure:
            record(statuses, failure.group(1), FAIL)
            pending = header = None
            continue
        if re.match(r"^test\S+ \([^)]+\)$", line):
            header = line
            pending = None
            continue
        m = re.match(r"^(.+?) \.\.\. (.*)$", line)
        if m and re.fullmatch(r"test\S+ \([^)]+\)", m.group(1)):
            header = None
        word = (m.group(2) if m else line).strip()
        name = m.group(1) if m else pending or header
        status = django_status(word)
        if m and status is None:
            pending = name
            continue
        if name is not None and status is not None:
            record(statuses, name, status)
            if header is not None:
                record(statuses, header, status)
            pending = None
            header = None
    return statuses


def record(statuses: dict[str, str], name: str, status: str) -> None:
    """Two tests can share a name, as sympy's do across files: the name gets
    the worse result, so a failure or a skip of either is not a pass."""
    rank = {FAIL: 0, SKIP: 1, XFAIL: 1, PASS: 2}
    previous = statuses.get(name)
    statuses[name] = status if previous is None or rank[status] < rank[previous] else previous


def django_status(word: str) -> str | None:
    if word in ("ok", "OK"):
        return PASS
    if word == "expected failure":
        return XFAIL
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
        record(statuses, m.group(1), {"ok": PASS, "f": XFAIL, "s": SKIP, "Skipped": SKIP}.get(m.group(2), FAIL))
    return statuses


def parse_pytest(log: str) -> dict[str, str]:
    """Read verbose progress and the `-rA` summary, including partial runs."""
    statuses = {}
    progress = re.split(r"^={3,}\s*(?:FAILURES|ERRORS|PASSES|warnings summary|short test summary info)\s*={3,}\s*$",
                        log, maxsplit=1, flags=re.M)[0]
    for m in re.finditer(r"^(\S+\.py::\S+) (PASSED|FAILED|ERROR|XFAIL|XPASS|SKIPPED)(?:\s|$)", progress, re.M):
        status = {"PASSED": PASS, "XFAIL": XFAIL, "SKIPPED": SKIP}.get(m.group(2), FAIL)
        record(statuses, m.group(1), status)
    for m in re.finditer(r"^(PASSED|FAILED|ERROR|XFAIL|XPASS|SKIPPED) (\S+)", log, re.M):
        if m.group(1) == "SKIPPED":
            continue  # `SKIPPED [1] path:line: reason` names no test.
        status = {"PASSED": PASS, "XFAIL": XFAIL}.get(m.group(1), FAIL)
        record(statuses, m.group(2), status)
    return statuses


PARSERS = {"django": parse_django, "sympy": parse_sympy, "pytest": parse_pytest}


def verdict(spec: dict, statuses: dict[str, str]) -> tuple[list[str], list[str]]:
    """Separate observed failures from required tests without a passing result."""
    problems = []
    unverified = []
    for category in ("fail_to_pass", "pass_to_pass"):
        for name in spec[category]:
            status = statuses.get(name)
            detail = f"{category.upper()} {name}: {status or 'not run or not named'}"
            if status == FAIL:
                problems.append(detail)
            elif status != PASS:
                unverified.append(detail)
    return problems, unverified


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


def save_log(spec: dict, log: str) -> None:
    Path(os.environ.get("TMPDIR", "/tmp"), f"{spec['instance_id']}.test.log").write_text(log)


def check(task_dir: Path) -> None:
    spec = load(task_dir)
    place_hidden(task_dir, spec)
    env = dict(os.environ)
    if spec.get("pythonpath") is not None:
        env["PYTHONPATH"] = str(Path.cwd() / spec["pythonpath"])
    command = [sys.executable if arg == "{python}" else arg for arg in spec["test_command"]]
    try:
        proc = subprocess.run(command, capture_output=True, text=True, env=env, timeout=spec.get("test_timeout_s", 600))
    except subprocess.TimeoutExpired as error:
        # What the runner printed before it hung, which may be bytes.
        partial = "".join(out.decode(errors="replace") if isinstance(out, bytes) else out
                          for out in (error.stdout, error.stderr) if out)
        save_log(spec, partial)
        parser = PARSERS.get(spec["log_format"])
        if parser is not None:
            failed = [name for name, status in parser(partial).items() if status == FAIL]
            expect(not failed, "tests failed before the timeout: " + ", ".join(failed) + f"\n{partial}")
        UNVERIFIED.append(f"the tests timed out: {' '.join(command)}\n{partial}")
        finish()
        return
    log = proc.stdout + proc.stderr
    save_log(spec, log)
    expect(proc.returncode == 0, f"the tests exited {proc.returncode}:\n{log[-3000:]}")
    parser = PARSERS.get(spec["log_format"])
    if parser is not None:
        statuses = parser(log)
        failed = [name for name, status in statuses.items() if status == FAIL]
        if proc.returncode == 0 and failed:
            expect(False, "the tests exited 0 but reported failures: " + ", ".join(failed))
        problems, unverified = verdict(spec, statuses)
        UNVERIFIED.extend(unverified)
        print(f"{sum(s == PASS for s in statuses.values())} of {len(statuses)} named results passed "
              "(a test can have multiple names)")
        expect(not problems, "the instance's tests fail:\n" + "\n".join(problems) + f"\n{log[-2500:]}")
    elif spec["log_format"] != "exit":
        UNVERIFIED.append(f"no parser for test log format {spec['log_format']!r}")
    # Not a condition of the check: the prompt asks for a commit.
    print(f"committed: {new_commit()}")
    finish()


def solve(task_dir: Path) -> None:
    spec = load(task_dir)
    subprocess.run(["git", "apply", str(task_dir / "fix.patch")], check=True)
    commit(f"Fix {spec['instance_id']}", *spec["patch_files"])
