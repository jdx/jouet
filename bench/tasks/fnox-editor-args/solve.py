import subprocess
from pathlib import Path

# The merged fix, including the broader edit features in the same PR.
subprocess.run(["git", "apply", str(Path(__file__).with_name("fix.patch"))], check=True)
