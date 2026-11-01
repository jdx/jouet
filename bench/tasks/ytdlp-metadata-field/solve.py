import subprocess
from pathlib import Path

# The upstream fix from yt-dlp/yt-dlp#14577.
subprocess.run(["git", "apply", str(Path(__file__).with_name("fix.patch"))], check=True)
