import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from _check import UNVERIFIED, dirty, expect, finish, head_sha, new_commit
from agent_tests import check_committed_tests, events_from, remaining, save_log


def check_format(path: str, added: bool, deadline: float) -> None:
    """Reject new formatting failures; an already unformatted base is unknown."""
    try:
        current = Path(path).read_bytes()
        formatted = subprocess.run(["gofmt"], check=False, input=current, capture_output=True,
                                   timeout=min(60, remaining(deadline)))
        if formatted.returncode:
            UNVERIFIED.append(f"gofmt could not check {path}")
            return
        if formatted.stdout == current:
            return
        if added:
            expect(False, f"gofmt fails on new file {path}")
            return
        base = subprocess.run(["git", "show", f"{head_sha()}:{path}"], check=False, capture_output=True,
                              timeout=min(30, remaining(deadline)))
        if base.returncode:
            UNVERIFIED.append(f"could not read base version of {path}")
            return
        original = subprocess.run(["gofmt"], check=False, input=base.stdout, capture_output=True,
                                  timeout=min(60, remaining(deadline)))
        if original.returncode or original.stdout != base.stdout:
            UNVERIFIED.append(f"gofmt fails or cannot check the base version of {path}")
        else:
            expect(False, f"gofmt fails on {path}")
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"gofmt could not check {path}: {error}")

def check_checkout(deadline: float) -> None:
    # Compile an independent external-package test against Cobra's public API.
    hidden = Path("jouet_bench_completion_test.go")
    created = False
    try:
        with hidden.open("x") as out:
            created = True
            out.write((Path(__file__).parent / "hidden" / "completion_test.go").read_text())
        try:
            result = subprocess.run(["go", "test", "-json", "-count=1", "-run", "^TestJouetCompletionKeepsArguments$", "."],
                                    check=False, capture_output=True, text=True, timeout=remaining(deadline))
        except subprocess.TimeoutExpired as error:
            output = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
            stderr = error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else error.stderr or ""
            events, _ = events_from(output)
            log = save_log(output, stderr)
            if any(e.get("Action") == "fail" and isinstance(e.get("Test"), str)
                   and (e["Test"] == "TestJouetCompletionKeepsArguments"
                        or e["Test"].startswith("TestJouetCompletionKeepsArguments/")) for e in events):
                expect(False, f"completion cases failed before timeout (full log: {log})")
            UNVERIFIED.append(f"completion cases timed out (full log: {log})")
        else:
            events, malformed = events_from(result.stdout)
            name = "TestJouetCompletionKeepsArguments"
            failures = [e for e in events if e.get("Action") == "fail" and isinstance(e.get("Test"), str)
                        and (e["Test"] == name or e["Test"].startswith(name + "/"))]
            if failures:
                expect(False, f"completion cases failed (full log: {save_log(result.stdout, result.stderr)})")
            elif (result.returncode or malformed
                  or not any(e.get("Action") == "run" and e.get("Test") == name for e in events)
                  or not any(e.get("Action") == "pass" and e.get("Test") == name for e in events)
                  or not any(e.get("Action") == "pass" and not e.get("Test") for e in events)
                  or any(e.get("Action") == "skip" and e.get("Test") for e in events)):
                UNVERIFIED.append(f"completion cases did not complete (full log: {save_log(result.stdout, result.stderr)})")
    finally:
        if created:
            hidden.unlink(missing_ok=True)
    added_result = subprocess.run(["git", "diff", "--name-only", "-z", "--diff-filter=A", head_sha(), "HEAD"],
                                  capture_output=True, text=True, check=True, timeout=remaining(deadline))
    added = set(added_result.stdout.split("\0"))
    changed_result = subprocess.run(["git", "diff", "--name-only", "-z", head_sha(), "HEAD"],
                                    capture_output=True, text=True, check=True, timeout=remaining(deadline))
    for path in sorted(filter(None, changed_result.stdout.split("\0"))):
        if path.endswith(".go") and Path(path).is_file():
            check_format(path, path in added, deadline)
    check_committed_tests(deadline)


def main() -> None:
    # Leave the 180-second outer limit time to collect diagnostics and clean up.
    deadline = time.monotonic() + 160
    root = Path.cwd()
    try:
        committed = new_commit()
        changes = dirty()
        status = subprocess.run(["git", "status", "--porcelain", "--untracked-files=normal"],
                                capture_output=True, text=True, check=True, timeout=min(30, remaining(deadline)))
    except (OSError, subprocess.SubprocessError) as error:
        UNVERIFIED.append(f"could not inspect the committed checkout: {error}")
        finish()
    expect(committed, "nothing was committed")
    expect(not changes, f"tracked changes were left uncommitted: {changes}")
    expect(not status.stdout, "changes were left uncommitted")
    if changes or status.stdout:
        finish()
    with tempfile.TemporaryDirectory(prefix="jouet-cobra-check-") as directory:
        checkout = Path(directory) / "work"
        try:
            subprocess.run(["git", "clone", "--quiet", "--no-hardlinks", "--", str(root), str(checkout)],
                           capture_output=True, text=True, check=True, timeout=min(60, remaining(deadline)))
            os.chdir(checkout)
            try:
                check_checkout(deadline)
            finally:
                os.chdir(root)
        except (OSError, subprocess.SubprocessError) as error:
            UNVERIFIED.append(f"could not verify the isolated checkout: {error}")
    finish()


if __name__ == "__main__":
    main()
