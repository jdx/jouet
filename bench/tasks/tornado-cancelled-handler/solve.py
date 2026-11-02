import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (tornadoweb/tornado#3695), with its test.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("web: Handle CancelledError from request handlers", "tornado/web.py", "tornado/test/web_test.py")
