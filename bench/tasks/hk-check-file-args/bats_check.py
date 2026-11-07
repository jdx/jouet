"""Verify the committed Bats regressions before installing hidden tests."""

import os
import re
import subprocess
import sys
from pathlib import Path

from _check import UNVERIFIED, expect, head_sha


def test_names(source: str) -> list[str]:
    return re.findall(r'^\s*@test\s+"([^"]+)"\s*\{', source, re.MULTILINE)


def check_bats() -> None:
    try:
        def paths(mode: str) -> set[str]:
            proc = subprocess.run(
                ["git", "diff", "--no-renames", "--name-only", "-z", f"--diff-filter={mode}",
                 head_sha(), "HEAD", "--", "test"],
                capture_output=True, check=True, timeout=30,
            )
            return {os.fsdecode(path) for path in proc.stdout.split(b"\0") if path}

        changed = {path for path in paths("AM") if path.endswith(".bats")}
        added_files = paths("A")
        removed = sorted(path for path in paths("D") if path.endswith(".bats"))
        if removed:
            UNVERIFIED.append(f"committed Bats files were removed or renamed: {', '.join(removed)}")
        if not changed:
            UNVERIFIED.append("no committed Bats regression test was changed")
            return
        env = {**os.environ, "PATH": f"{Path.cwd() / 'target/debug'}{os.pathsep}{os.environ.get('PATH', '')}"}
        env.pop("GIT_CONFIG_GLOBAL", None)
        found_new = False
        for path in sorted(changed):
            current = subprocess.run(["git", "show", f"HEAD:{path}"], capture_output=True, text=True,
                                     check=True, timeout=30).stdout
            original = "" if path in added_files else subprocess.run(
                ["git", "show", f"{head_sha()}:{path}"], capture_output=True, text=True, check=True, timeout=30,
            ).stdout
            before, after = test_names(original), test_names(current)
            if (len(set(before)) != len(before) or len(set(after)) != len(after)
                    or not set(before).issubset(after)):
                UNVERIFIED.append(f"starting Bats tests are missing, renamed, or ambiguous in {path}")
                continue
            new = [name for name in after if name not in set(before)]
            found_new |= bool(new)
            try:
                run = subprocess.run(["test/bats/bin/bats", "--tap", path], capture_output=True, text=True,
                                     timeout=300, check=False, env=env)
            except subprocess.TimeoutExpired as error:
                output = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
                print(output, end="")
                failed = re.findall(r"^not ok \d+\b([^\n]*)$", output, re.MULTILINE)
                if any(name.strip() in new for name in failed):
                    expect(False, f"a committed Bats regression failed before the suite timed out in {path}")
                else:
                    UNVERIFIED.append(f"Bats suite timed out for {path}")
                continue
            print(run.stdout, end="")
            if run.stderr:
                print(run.stderr, end="", file=sys.stderr)
            results = re.findall(r"^(not ok|ok) (\d+)\b([^\n]*)$", run.stdout, re.MULTILINE)
            failed = [result[2].strip() for result in results if result[0] == "not ok"]
            if any(name in new for name in failed):
                expect(False, f"a committed Bats regression failed in {path}")
                continue
            plans = re.findall(r"^1\.\.(\d+)\s*$", run.stdout, re.MULTILINE)
            names = [result[2].strip() for result in results]
            complete = (len(plans) == 1 and int(plans[0]) > 0 and len(results) == int(plans[0])
                        and [int(result[1]) for result in results] == list(range(1, len(results) + 1))
                        and all(names.count(name) == 1 for name in after)
                        and not any(re.search(r"#\s*(skip|todo)\b", result[2], re.IGNORECASE)
                                    for result in results))
            if run.returncode != 0 or failed or not complete:
                UNVERIFIED.append(f"Bats results were incomplete or could not be verified for {path}")
        if not found_new:
            UNVERIFIED.append("no new standard-form Bats regression test was committed")
    except (OSError, subprocess.SubprocessError) as error:
        UNVERIFIED.append(f"could not verify committed Bats tests: {error}")
