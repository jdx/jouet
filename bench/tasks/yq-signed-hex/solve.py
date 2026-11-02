import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (mikefarah/yq#2749), with its tests.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix: parse signed hex and octal integers",
    "pkg/yqlib/lib.go",
    "pkg/yqlib/json_test.go",
    "pkg/yqlib/lib_test.go",
)
