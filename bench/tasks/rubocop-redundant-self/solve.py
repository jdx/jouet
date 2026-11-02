import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit("fix: keep self on splat assignment targets",
       "lib/rubocop/cop/style/redundant_self.rb",
       "spec/rubocop/cop/style/redundant_self_spec.rb",
       "changelog/fix_false_positives_in_style_redundant_self_with_a_splat_in_multiple_assignment_20261024215147.md")
