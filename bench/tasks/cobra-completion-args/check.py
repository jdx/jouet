import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit  # noqa: E402

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
finish()
