import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# Exercise the public postprocessor with synthetic metadata and no network.
program = r'''
import json
from yt_dlp import YoutubeDL
from yt_dlp.postprocessor import MetadataFromFieldPP

ydl = YoutubeDL({"quiet": True, "ignoreconfig": True})
cases = [
    ("playlist_autonumber:track", {"playlist_autonumber": 3}, "track"),
    ("title:description", {"title": "first\nsecond"}, "description"),
    ("title:%(artist)s - %(track)s", {"title": "Name - Song"}, "artist"),
    (r"title:(?P<artist>.+) - (?P<track>.+)", {"title": "Name - Song"}, "track"),
]
result = []
for action, info, key in cases:
    _, changed = MetadataFromFieldPP(ydl, [action]).run(info)
    result.append(changed.get(key))
print(json.dumps(result))
'''
run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                     env={**os.environ, "PYTHONPATH": str(Path.cwd())}, timeout=60)
try:
    values = json.loads(run.stdout)
except json.JSONDecodeError:
    values = None
expect(run.returncode == 0 and values == ["3", "first\nsecond", "Name", "Song"],
       f"metadata parsing did not preserve all cases: {run.stdout}\n{run.stderr}")
print(f"committed: {new_commit()}")
finish()
