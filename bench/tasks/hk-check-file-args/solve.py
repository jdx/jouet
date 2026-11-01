import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (jdx/hk#1754), with its tests.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(check): resolve file arguments from the directory hk was run in",
    "src/hook.rs",
    "src/hook_options.rs",
    "test/check.bats",
    "test/top_level_exclude.bats",
)
