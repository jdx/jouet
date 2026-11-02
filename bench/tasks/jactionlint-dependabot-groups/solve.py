import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The parser change from jdx/jactionlint#314.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("fix: distinguish grouped Dependabot updates", "dependabot_parse.go")
