import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (jdx/usage#1583), with its test.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(cli): order command help sections by display_order",
    "argv/src/help.rs",
    "conformance/tests/heading_help.rs",
    "docs/rust/help.md",
    "lib/src/docs/models.rs",
)
