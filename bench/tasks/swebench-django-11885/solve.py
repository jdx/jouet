import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _swebench import solve  # noqa: E402

solve(Path(__file__).resolve().parent)
