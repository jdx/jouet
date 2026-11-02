import subprocess
from pathlib import Path

# The upstream fix from astral-sh/ruff#29366, applied to its parent commit.
subprocess.run(["git", "apply", str(Path(__file__).with_name("fix.patch"))], check=True)
changed = subprocess.run(["git", "diff", "--name-only", "-z"], capture_output=True, check=True).stdout
paths = [p.decode() for p in changed.split(b"\0") if p]
subprocess.run(["git", "add", "--", *paths], check=True)
subprocess.run(["git", "commit", "-q", "-m", "fix PERF iterable rewrites", "--", *paths], check=True)
