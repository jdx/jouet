import subprocess
from pathlib import Path

# The original fix from sharkdp/fd#2193. Its merged revision lost the
# reverse-order behavior; this earlier commit preserves it.
subprocess.run(["git", "apply", str(Path(__file__).with_name("fix.patch"))], check=True)
