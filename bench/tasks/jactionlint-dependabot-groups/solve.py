import subprocess
import sys
from pathlib import Path
from shutil import copyfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The parser change from jdx/jactionlint#314.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
copyfile(Path(__file__).with_name("reference_test.go"), "jouet_bench_dependabot_test.go")
commit("fix: distinguish grouped Dependabot updates", "dependabot_parse.go", "jouet_bench_dependabot_test.go")
