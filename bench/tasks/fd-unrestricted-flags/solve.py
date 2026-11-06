import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit

# The original fix from sharkdp/fd#2193. Its merged revision lost the
# reverse-order behavior; this earlier commit preserves it.
subprocess.run(["git", "apply", str(Path(__file__).with_name("fix.patch"))], check=True)
commit("fix: preserve independent unrestricted overrides", "CHANGELOG.md", "src/cli.rs", "src/main.rs", "tests/tests.rs")
