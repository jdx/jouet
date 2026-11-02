import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# Exercise the public parser without replacing any of the agent's tests.
run = subprocess.run(["go", "run", str(Path(__file__).parent / "check.go")],
                     capture_output=True, text=True, timeout=540)
expect(run.returncode == 0, f"Dependabot cases failed:\n{run.stdout}\n{run.stderr}")
print(f"committed: {new_commit()}")
finish()
