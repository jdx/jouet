import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, head_sha, new_commit  # noqa: E402


def check_format(path: str, added: bool) -> None:
    """Reject new formatting failures; an already unformatted base is unknown."""
    try:
        current = Path(path).read_bytes()
        formatted = subprocess.run(["gofmt"], input=current, capture_output=True, timeout=60)
        if formatted.returncode:
            UNVERIFIED.append(f"gofmt could not check {path}")
            return
        if formatted.stdout == current:
            return
        if added:
            expect(False, f"gofmt fails on new file {path}")
            return
        base = subprocess.run(["git", "show", f"{head_sha()}:{path}"], capture_output=True, timeout=30)
        if base.returncode:
            UNVERIFIED.append(f"could not read base version of {path}")
            return
        original = subprocess.run(["gofmt"], input=base.stdout, capture_output=True, timeout=60)
        if original.returncode or original.stdout != base.stdout:
            UNVERIFIED.append(f"gofmt fails or cannot check the base version of {path}")
        else:
            expect(False, f"gofmt fails on {path}")
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"gofmt could not check {path}: {error}")

expect(new_commit(), "nothing was committed")
expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")

# Compile an independent external-package test against Cobra's public API.
hidden = Path("jouet_bench_completion_test.go")
created = False
try:
    with hidden.open("x") as out:
        created = True
        out.write((Path(__file__).parent / "hidden" / "completion_test.go").read_text())
    result = subprocess.run(["go", "test", ".", "-run", "^TestJouetCompletionKeepsArguments$", "-count=1"],
                            capture_output=True, text=True, timeout=150)
    expect(result.returncode == 0, f"completion cases failed:\n{result.stdout}\n{result.stderr}")
finally:
    if created:
        hidden.unlink(missing_ok=True)
added_result = subprocess.run(["git", "diff", "--name-only", "--diff-filter=A", head_sha(), "HEAD"],
                              capture_output=True, text=True, check=True)
added = set(added_result.stdout.splitlines())
for path in sorted(committed_files()):
    if path.endswith(".go") and Path(path).is_file():
        check_format(path, path in added)
finish()
