import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, expect, finish, head_sha, new_commit


def tail(output: str) -> str:
    if len(output) > 8000:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", prefix="jouet-usage-check-",
                                         suffix=".log", delete=False) as file:
            file.write(output)
        return f"... {len(output) - 8000} earlier characters omitted (full log: {file.name})\n" + output[-8000:]
    return output


def run(target: str, label: str, deadline: float, checkout: Path) -> None:
    env = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": str(checkout), "CARGO_TERM_COLOR": "never"}
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        UNVERIFIED.append(f"{label} could not run within the check budget")
        return
    try:
        proc = subprocess.run(
            ["cargo", "test", "-p", "usage-conformance", "--test", target],
            check=False, cwd=checkout, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, env=env, timeout=remaining,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        if isinstance(error, subprocess.TimeoutExpired) and error.stdout:
            output = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout
            if re.search(r"^test [^\r\n]+ \.\.\. FAILED\r?$", output, re.MULTILINE):
                expect(False, f"{label} reported a failing test before it timed out:\n{tail(output)}")
            else:
                print(tail(output))
        UNVERIFIED.append(f"{label} could not complete: {error}")
        return
    output = proc.stdout
    reported_failure = bool(re.search(r"^test [^\r\n]+ \.\.\. FAILED\r?$", output, re.MULTILINE))
    if reported_failure:
        expect(False, f"{label} reported a failing test:\n{tail(output)}")
    running = re.findall(r"^running (\d+) tests?$", output, re.MULTILINE)
    summaries = re.findall(
        r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"(\d+) measured; (\d+) filtered out;", output, re.MULTILINE,
    )
    if len(running) != 1 or len(summaries) != 1:
        UNVERIFIED.append(f"{label} did not produce a complete test result:\n{tail(output)}")
        return
    status, passed, failed, ignored, measured, filtered = summaries[0]
    passed, failed, ignored, measured, filtered = map(int, (passed, failed, ignored, measured, filtered))
    if passed + failed + ignored != int(running[0]):
        UNVERIFIED.append(f"{label} reported inconsistent test counts:\n{tail(output)}")
        return
    if failed and not reported_failure:
        expect(False, f"{label} failed:\n{tail(output)}")
    if ignored or measured or filtered or not passed + failed:
        UNVERIFIED.append(f"{label} did not run a complete nonempty test target:\n{tail(output)}")
        return
    if not failed and (proc.returncode != 0 or status != "ok" or reported_failure):
        UNVERIFIED.append(f"{label} exited without a consistent passing result:\n{tail(output)}")
    elif not failed:
        print(f"{label}: {passed} passed")


def check_phase(tests: list[Path], deadline: float, *, hidden: bool) -> None:
    with tempfile.TemporaryDirectory(prefix="jouet-usage-check-") as directory:
        checkout = (Path(directory) / "work").resolve()
        try:
            created = subprocess.run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                                     capture_output=True, text=True, timeout=30, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            UNVERIFIED.append(f"could not create test checkout: {error}")
            return
        if created.returncode:
            UNVERIFIED.append(f"could not create test checkout: {created.stderr}")
            return
        try:
            root = checkout / "conformance/tests"
            if not root.is_dir() or any(parent.is_symlink() for parent in (root, *root.parents)):
                UNVERIFIED.append("cannot safely access conformance tests")
                return
            if hidden:
                try:
                    with tempfile.NamedTemporaryFile(prefix="jouet_hidden_", suffix=".rs", dir=root, delete=False) as file:
                        target = Path(file.name)
                        file.write((Path(__file__).parent / "hidden" / "heading_help.rs").read_bytes())
                    run(target.stem, "hidden conformance tests", deadline, checkout)
                except OSError as error:
                    UNVERIFIED.append(f"cannot create hidden conformance test: {error}")
            else:
                for path in tests:
                    target = checkout / path
                    if (not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_-]*", path.stem) or not target.is_file()
                            or any(parent.is_symlink() for parent in (target, *target.parents))):
                        UNVERIFIED.append(f"cannot safely run changed test target {path}")
                        continue
                    run(path.stem, f"committed test {path}", deadline, checkout)
        finally:
            try:
                removed = subprocess.run(["git", "worktree", "remove", "--force", str(checkout)],
                                         capture_output=True, text=True, timeout=30, check=False)
                if removed.returncode:
                    UNVERIFIED.append(f"could not remove test checkout: {removed.stderr}")
            except (OSError, subprocess.TimeoutExpired) as error:
                UNVERIFIED.append(f"could not remove test checkout: {error}")


def check() -> None:
    # Leave the outer 1,200-second budget room for both worktree phases and cleanup.
    deadline = time.monotonic() + 1000
    committed = new_commit()
    expect(committed, "nothing was committed")
    status = subprocess.run(["git", "status", "--porcelain", "--untracked-files=normal"],
                            capture_output=True, text=True, check=True).stdout
    expect(not status, "changes remain uncommitted")
    root = Path("conformance/tests")
    changed = subprocess.run(
        ["git", "diff", "--name-only", "-z", "--no-renames", "--diff-filter=ACMRT", head_sha(), "HEAD", "--", str(root)],
        capture_output=True, check=True,
    ).stdout
    tests = sorted(Path(os.fsdecode(path)) for path in changed.split(b"\0") if path
                   and Path(os.fsdecode(path)).parent == root and Path(os.fsdecode(path)).suffix == ".rs")
    if tests:
        check_phase(tests, deadline, hidden=False)
    else:
        UNVERIFIED.append("no changed conformance test target to verify")
    check_phase([], deadline, hidden=True)
    print(f"committed: {committed}")


if __name__ == "__main__":
    check()
    finish()
