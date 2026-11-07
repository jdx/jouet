"""Verify committed Go tests independently of the hidden behavior probe."""

import json
import re
import subprocess
import tempfile
from pathlib import Path

from _check import UNVERIFIED, expect, head_sha


def save_log(stdout: str, stderr: str) -> str:
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", prefix="jouet-cobra-check-",
                                     suffix=".log", delete=False) as log:
        log.write(stdout + stderr)
    return log.name


def events_from(output: str) -> tuple[list[dict], bool]:
    events = []
    malformed = False
    for line in output.splitlines():
        try:
            event = json.loads(line)
            if not isinstance(event, dict):
                malformed = True
                continue
            events.append(event)
        except ValueError:
            malformed = True
    return events, malformed


def check_committed_tests() -> None:
    try:
        changed = subprocess.run(
            ["git", "diff", "--name-only", "-z", "--no-renames", "--diff-filter=ACMRT", head_sha(), "HEAD"],
            capture_output=True, text=True, check=True, timeout=30,
        ).stdout.split("\0")
        files = sorted(Path(path) for path in changed if path.endswith("_test.go"))
        if not files:
            UNVERIFIED.append("no changed Go test file could be verified")
            return
        packages = {}
        added = set()
        for path in files:
            if not path.is_file() or any(part.is_symlink() for part in (path, *path.parents)):
                UNVERIFIED.append(f"cannot safely inspect committed test file {path}")
                continue
            package = "." if path.parent == Path(".") else f"./{path.parent.as_posix()}"
            if package not in packages:
                listed = subprocess.run(["go", "list", "-json", package], capture_output=True, text=True,
                                        check=False, timeout=60)
                if listed.returncode:
                    UNVERIFIED.append(f"cannot list Go package {package}: {listed.stderr}")
                    continue
                packages[package] = json.loads(listed.stdout)
            info = packages[package]
            if path.name not in info.get("TestGoFiles", []) + info.get("XTestGoFiles", []):
                UNVERIFIED.append(f"changed test file is not selected by Go: {path}")
                continue
            diff = subprocess.run(["git", "diff", "--unified=0", head_sha(), "HEAD", "--", str(path)],
                                  capture_output=True, text=True, check=True, timeout=30).stdout
            added.update((info["ImportPath"], name) for name in
                         re.findall(r"^\+func\s+(Test[A-Za-z0-9_]+)\s*\(", diff, re.MULTILINE))
        if not added:
            UNVERIFIED.append("no newly added Go test function could be verified")
        for package, info in packages.items():
            try:
                run = subprocess.run(["go", "test", "-json", "-count=1", package], capture_output=True,
                                     text=True, check=False, timeout=300)
            except subprocess.TimeoutExpired as error:
                output = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
                stderr = error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else error.stderr or ""
                events, _ = events_from(output)
                log = save_log(output, stderr)
                if any(e.get("Action") == "fail" and e.get("Test") for e in events):
                    expect(False, f"committed Go tests failed before timeout in {package} (full log: {log})")
                UNVERIFIED.append(f"committed Go tests timed out in {package} (full log: {log})")
                continue
            events, malformed = events_from(run.stdout)
            failed = [e for e in events if e.get("Action") == "fail" and e.get("Test")]
            if failed:
                expect(False, f"committed Go tests failed in {package} (full log: {save_log(run.stdout, run.stderr)})")
                continue
            expected = {name for pkg, name in added if pkg == info["ImportPath"]}
            started = {e.get("Test") for e in events if e.get("Action") == "run" and e.get("Package") == info["ImportPath"]}
            passed = {e.get("Test") for e in events if e.get("Action") == "pass" and e.get("Package") == info["ImportPath"]}
            skipped = any(e.get("Action") == "skip" and e.get("Test") for e in events)
            package_pass = any(e.get("Action") == "pass" and not e.get("Test")
                               and e.get("Package") == info["ImportPath"] for e in events)
            if (run.returncode or malformed or skipped or not package_pass
                    or not expected.issubset(started & passed) or not passed):
                UNVERIFIED.append(f"committed Go tests were incomplete in {package} (full log: {save_log(run.stdout, run.stderr)})")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        UNVERIFIED.append(f"could not verify committed Go tests: {error}")
