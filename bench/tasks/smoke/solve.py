import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

path = Path("wordcount.py")
path.write_text(path.read_text().replace('text.split(" ")', "text.split()"))
commit("fix: count words split by any whitespace", "wordcount.py")
