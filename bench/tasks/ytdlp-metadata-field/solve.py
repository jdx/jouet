import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream fix from yt-dlp/yt-dlp#14577.
subprocess.run(["git", "apply", str(Path(__file__).with_name("fix.patch"))], check=True)
commit("fix: parse a single metadata field", "README.md", "test/test_postprocessors.py",
       "yt_dlp/postprocessor/metadataparser.py")
